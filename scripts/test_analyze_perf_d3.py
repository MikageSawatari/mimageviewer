#!/usr/bin/env python3
"""Reference-only D3 diagnostics; no native application or external packages."""

from __future__ import annotations

import contextlib
import io
import json
import sys
import unittest
from unittest import mock

from analyze_perf import analyze_page_turn_d3, main
from test_analyze_perf import delivery_events, writable_test_directory


def animation(t, kind, operation="one", context="ViewerContextId(0)", window="main", **fields):
    return {"cat": "page_turn_animation", "kind": kind, "t": t,
            "operation_id": operation, "context": context, "window": window, **fields}


def curl(t, frame, output_ms, **fields):
    return animation(t, "frame", frame=frame, output_motion_elapsed_ms=output_ms,
                     reason="curl_submitted", **fields)


class PageTurnD3Tests(unittest.TestCase):
    def test_per_operation_unique_curls_and_precise_clock_exclude_canonical(self):
        events = [animation(1, "opportunity"),
                  animation(1.005, "start", frame=10, reason="curl_submitted",
                            accept_to_first_curl_ms=4.125, output_motion_elapsed_ms=0),
                  curl(1.006, 10, 0, accept_to_first_curl_ms=4.125),
                  curl(1.017, 11, 10.125), curl(1.017, 11, 10.125),
                  curl(1.05, 12, 40.625),
                  animation(1.3, "finish", frame=13, reason="canonical_submitted",
                            output_motion_elapsed_ms=290, motion_max_gap_ms=30.4)]
        operation = analyze_page_turn_d3(list(reversed(events)))["operations"][0]
        self.assertEqual(operation["curl_frames"], 3)
        self.assertEqual(operation["accept_to_first_curl_ms"], 4.125)
        self.assertEqual(operation["curl_intervals_ms"], [10.125, 30.5])
        self.assertEqual(operation["curl_interval_max_ms"], 30.5)
        self.assertAlmostEqual(operation["curl_interval_p95_ms"], 29.48125)
        self.assertEqual(operation["reported_motion_max_gap_ms"], 30.4)
        self.assertTrue(operation["finished"])

    def test_context_window_operation_identity_and_incomplete_outcomes(self):
        rows = [animation(1, "opportunity"), curl(1.01, 1, 0),
                animation(1.02, "cancel", reason="focus_lost"),
                animation(2, "opportunity", context="ViewerContextId(1)"),
                animation(2.001, "skip", context="ViewerContextId(1)", reason="not_ready"),
                animation(3, "opportunity", window="detached"),
                curl(3.01, 1, 0, window="detached"),
                animation(4, "opportunity", operation="two")]
        report = analyze_page_turn_d3(rows)
        self.assertEqual(len(report["operations"]), 4)
        cancelled, skipped, incomplete, zero = report["operations"]
        self.assertEqual(cancelled["terminal_reasons"], ["focus_lost"])
        self.assertTrue(cancelled["cancelled"])
        self.assertEqual(cancelled["curl_frames"], 1)
        self.assertIsNone(cancelled["curl_interval_p95_ms"])
        self.assertTrue(skipped["skipped"])
        self.assertEqual(skipped["curl_frames"], 0)
        self.assertIsNone(skipped["accept_to_first_curl_ms"])
        self.assertFalse(incomplete["finished"])
        self.assertEqual(zero["curl_frames"], 0)
        self.assertEqual(report["status"], "reference")

    def test_legacy_timestamps_and_nonmonotonic_precise_clock_are_explicit(self):
        rows = [animation(1, "opportunity"),
                animation(1.01, "frame", frame=1, reason="curl_submitted"),
                animation(1.02, "frame", frame=2, reason="curl_submitted")]
        operation = analyze_page_turn_d3(rows)["operations"][0]
        self.assertEqual(operation["curl_interval_basis"], "event_t_ms")
        self.assertAlmostEqual(operation["curl_interval_max_ms"], 10)
        self.assertAlmostEqual(operation["accept_to_first_curl_ms"], 10)
        self.assertEqual(operation["accept_latency_basis"], "opportunity_to_first_curl_event_t")
        rows[1]["output_motion_elapsed_ms"] = 5
        rows[2]["output_motion_elapsed_ms"] = 0
        report = analyze_page_turn_d3(rows)
        self.assertTrue(any("nonmonotonic" in reason for reason in report["warnings"]))
        self.assertEqual(report["operations"][0]["curl_interval_basis"], "event_t_ms")

    def test_start_is_only_a_fallback_for_traces_without_frame_receipts(self):
        start = animation(1.01, "start", reason="curl_submitted")
        receipt = animation(1.011, "frame", reason="curl_submitted")
        self.assertEqual(analyze_page_turn_d3([start, receipt])["operations"][0]["curl_frames"], 1)
        self.assertEqual(analyze_page_turn_d3([start])["operations"][0]["curl_frames"], 1)

    def test_d3_stages_are_observations_not_readiness_or_nearest_operation(self):
        rows = [
            {"cat": "fs", "kind": "final_prepare_cpu_ready", "context": 5, "t": .1},
            {"cat": "fs", "kind": "final_prepare_registered", "texture": "Managed(7)", "t": .2},
            {"cat": "gpu", "kind": "texture_delivery_submitted", "texture": "Managed(7)",
             "renderer": 2, "batch": 3, "bytes": 40, "t": .3},
            {"cat": "fs", "kind": "final_prepare_native_submitted", "native_key": "exact-key",
             "source_texture_id": "Managed(7)", "output_texture_id": "User(1)", "renderer": 2, "t": .4},
        ]
        rows += [{"cat": "fs", "kind": "final_prepare_native_lookup", "outcome": outcome, "t": .5}
                 for outcome in ("hit", "miss", "not_needed", "no_renderer")]
        rows += [{"cat": "fs", "kind": "final_prepare_gpu_deferred", "reason": "budget", "t": .6},
                 {"cat": "fs", "kind": "final_prepare_gpu_deferred", "stage": "native",
                  "reason": "budget", "t": .7}]
        report = analyze_page_turn_d3(rows)
        self.assertEqual(report["d3_event_counts"], dict(cpu_ready=1, registered=1,
                         texture_delivery_submitted=1, native_submitted=1, native_lookup=4, gpu_deferred=2))
        self.assertEqual(report["native_lookup_outcomes"], dict(hit=1, miss=1, not_needed=1, no_renderer=1))
        self.assertEqual(report["gpu_deferred_reasons"], [
            dict(stage="final", reason="budget", count=1), dict(stage="native", reason="budget", count=1)])
        self.assertEqual(report["d3_observations"][3]["native_key"], "exact-key")
        self.assertEqual(report["operations"], [])
        self.assertEqual(report["status"], "reference")
        self.assertEqual(report["delivery"]["status"], "CannotJudge")

    def test_bad_identity_time_and_absent_stages_do_not_create_success(self):
        report = analyze_page_turn_d3([animation(float("nan"), "opportunity"),
                                       animation(1, "frame", context=None)])
        self.assertEqual(report["operations"], [])
        self.assertEqual(report["d3_event_counts"]["native_submitted"], 0)
        self.assertEqual(report["native_lookup_outcomes"], dict(hit=0, miss=0, not_needed=0))
        self.assertEqual(len(report["warnings"]), 3)
        self.assertEqual(report["status"], "reference")

    def test_cli_json_is_reference_even_with_complete_delivery(self):
        rows = delivery_events([animation(1, "opportunity"), curl(1.01, 1, 0)])
        with writable_test_directory() as directory:
            source, output = directory / "perf.jsonl", directory / "report.json"
            source.write_text("".join(json.dumps(e) + "\n" for e in rows), encoding="utf-8")
            stream = io.StringIO()
            with mock.patch.object(sys, "argv", ["analyze_perf.py", str(source), "page-turn-d3",
                                                 "--json-out", str(output)]), contextlib.redirect_stdout(stream):
                with self.assertRaises(SystemExit) as result:
                    main()
            self.assertEqual(result.exception.code, 0)
            report = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(report["status"], "reference")
            self.assertEqual(report["delivery"]["status"], "complete")
            self.assertIn("not a formal", stream.getvalue())
            self.assertIn("curl_frames=1", stream.getvalue())
            self.assertIn("not GPU completion", stream.getvalue())


if __name__ == "__main__":
    unittest.main()
