#!/usr/bin/env python3
"""Synthetic Phase 2 regressions. No App process or desktop interaction."""
from __future__ import annotations

import contextlib
import io
import json
import math
import shutil
import subprocess
import sys
import unittest
import uuid
from pathlib import Path

from analyze_perf import analyze_idle_health
from release_check_analysis import (
    REQUIRED_CASES, analyze_case_evidence, analyze_idle_case, analyze_perf_smoke,
    convert_clock_window, finish, load_events_strict, main, release_record, report,
    summarize_suite,
)

PID = 42
OS_START = 1_700_000_000_000


def session() -> dict:
    return {"cat": "session", "kind": "start", "t": 5.251, "pid": PID,
            "wall_unix_ms": OS_START + 12_250, "wall_t": 5.25,
            "process_start_unix_ms": OS_START}


def operations() -> dict:
    return {"pid": PID, "folder_open": {"completed": True},
            "navigation": [{"receipt": True, "before_generation": n + 10, "after_generation": n + 11,
                            "before_path": f"folder{n}", "after_path": f"folder{n + 1}",
                            "start_t": 5.3 + n * .02, "end_t": 5.31 + n * .02} for n in range(5)],
            "search": {"ctrl_g_receipt": True, "text_receipt": True, "completed": True,
                       "expected_hits": ["a", "b"], "actual_hits": ["b", "a"],
                       "start_t": 5.4, "end_t": 5.5, "expected_query": "fixture-query", "actual_query": "fixture-query"},
            "observation": {"root_hwnd": 123},
            "exit": {"on_exit": True, "flush_complete": True, "process_exited": True, "forced": False},
            "cache": {"mode": "cached", "opens": [{"folder_path": "fixture", "items_gen": 3,
                                                     "target_keys": ["dir::fixture/child"]},
                                                    *[{"folder_path": f"folder{n}", "items_gen": n + 10,
                                                       "target_keys": [f"folder{n}/image.png"]} for n in range(6)]]},
            "fixture_manifest": {"perf": {mode: {"many_folders": "fixture",
                                                     "navigation_order": [f"folder{n}" for n in range(6)]}
                                           for mode in ("cached", "uncached")}}}


def metadata() -> dict:
    return {"source": "synthetic", "exe_sha256": "synthetic", "fixture_version": "synthetic",
            "environment": "synthetic", "run_path": "synthetic", "approval_scope": "synthetic",
            "thresholds_version": "synthetic"}


def perf_events(gaps: dict[int, float] | None = None, actions: dict[int, str] | None = None,
                pre_grid_ms: float = .2, frame_count: int = 202) -> list[dict]:
    gaps, actions = gaps or {}, actions or {}
    events, t = [session(), {"cat": "qa", "kind": "cache_read", "key": "dir::fixture/child",
                            "items_gen": 3, "idx": 0, "from_cache": True, "t": 5.6}], 6.0
    for n in range(5):
        events.append({"cat": "qa", "kind": "key_materialized", "t": 5.305 + n * .02,
                       "key": "Down", "ctrl": True, "shift": False, "alt": False,
                       "pressed": True, "repeat": False, "target_viewport": "ROOT", "target_hwnd": 123})
    for n in range(6):
        events.append({"cat": "qa", "kind": "cache_read", "t": 5.6, "key": f"folder{n}/image.png",
                       "items_gen": n + 10, "from_cache": True})
    events.extend([{"cat": "qa", "kind": "key_materialized", "t": 5.42,
                    "key": "G", "ctrl": True, "shift": False, "alt": False,
                    "pressed": True, "repeat": False, "target_viewport": "ROOT", "target_hwnd": 123},
                   {"cat": "qa", "kind": "text", "t": 5.43, "text": "fixture-query", "root_hwnd": 123}])
    for n in range(frame_count):
        frame = {"cat": "frame", "kind": "begin", "t": t, "n": n + 1}
        if n:
            frame.update({"prev_update_ms": 1, "prev_update_start_t": previous_t,
                          "prev_update_end_t": previous_t + .001})
        events.extend([frame, {"cat": "ui", "kind": "tail_repaint", "n": n + 1, "t": t + .001,
                               "action": actions.get(n, "request_repaint")},
                       {"cat": "ui", "kind": "pre_grid_breakdown", "n": n + 1,
                        "t": t + .001, "total_ms": pre_grid_ms, "poll_ms": pre_grid_ms}])
        previous_t, t = t, t + gaps.get(n, .005)
    events.append({"cat": "qa", "kind": "on_exit", "case_id": "perf-cached", "t": t + 1,
                   "flush_complete": True})
    return events


def frame_time(events: list[dict], n: int) -> float:
    return next(e["t"] for e in events if e.get("cat") == "frame" and e.get("n") == n + 1)


def slow_update(events: list[dict], n: int, ms: float, **stages: float) -> None:
    start = frame_time(events, n)
    for e in events:
        if e.get("cat") == "frame" and e.get("n") == n + 2:
            e.update({"prev_update_ms": ms, "prev_update_start_t": start,
                      "prev_update_end_t": start + ms / 1000})
    events.append({"cat": "ui", "kind": "update_breakdown", "n": n + 1, "t": start + ms / 1000,
                   "start_t": start, "end_t": start + ms / 1000, "total_ms": ms, **stages})


def perf(events: list[dict], evidence: dict | None = None) -> dict:
    return analyze_perf_smoke(events, expected_pid=PID, case_id="perf-cached",
                              evidence=operations() if evidence is None else evidence)


@contextlib.contextmanager
def test_directory():
    base = Path(__file__).resolve().parent.parent / "target" / "test-release-check-analysis"
    base.mkdir(parents=True, exist_ok=True)
    path = base / uuid.uuid4().hex
    path.mkdir()
    try:
        yield path
    finally:
        shutil.rmtree(path, ignore_errors=True)


class PerfTests(unittest.TestCase):
    def test_complete_natural_drawn_samples_pass(self):
        result = perf(perf_events())
        self.assertEqual(result["verdict"], "PASS", result)
        self.assertEqual(result["metrics"]["drawn"]["n"], 201)
        self.assertEqual(result["evidence"]["ai"]["status"], "AI未実施")

    def test_sleep_is_not_a_drawn_sample_or_hitch_failure(self):
        result = perf(perf_events({0: 16}, {0: "none"}))
        self.assertEqual(result["verdict"], "PASS")
        self.assertEqual(result["metrics"]["drawn"]["n"], 200)
        self.assertEqual(result["metrics"]["gap_categories"], {"normal-sleep": 1})

    def test_scheduled_idle_upgrade_requires_actual_delay(self):
        events = perf_events({0: .501}, {0: "request_repaint_after_idle_upgrade"})
        next(e for e in events if e.get("kind") == "tail_repaint" and e["n"] == 1)["idle_upgrade_delay_ms"] = 500
        self.assertEqual(perf(events)["verdict"], "PASS")
        next(e for e in events if e.get("kind") == "tail_repaint" and e["n"] == 1)["idle_upgrade_delay_ms"] = 20
        self.assertEqual(perf(events)["verdict"], "FAIL")

    def test_v450_tail_none_does_not_discard_578ms_scan_update(self):
        events = perf_events({0: .591}, {0: "none"})
        slow_update(events, 0, 578, startup_and_setup_ms=575)
        events.extend([{"cat": "nav", "kind": "load_folder_begin", "t": 6, "seq": 7},
                       {"cat": "nav", "kind": "lf_scan", "t": 6.572, "seq": 7, "ms": 572,
                        "folders": 522, "pre_scanned": False},
                       {"cat": "nav", "kind": "load_folder_end", "t": 6.578, "seq": 7}])
        result = perf(events)
        self.assertEqual(result["verdict"], "PASS_WITH_WARNINGS", result)
        self.assertEqual(result["metrics"]["gap_categories"], {"known-startup-scan": 1})
        self.assertEqual(result["metrics"]["slow_update_count"], 1)
        self.assertEqual(result["evidence"]["slow_updates"][0]["lf_scan_ms"], 572)
        self.assertEqual(result, perf(list(reversed(events))))

    def test_time_small_or_scan_during_later_operation_is_not_startup_waiver(self):
        events = perf_events({3: .591}, {3: "none"})
        slow_update(events, 3, 578, startup_and_setup_ms=575)
        events.extend([{"cat": "nav", "kind": "load_folder_begin", "t": 5.3, "seq": 1},
                       {"cat": "nav", "kind": "load_folder_end", "t": 5.5, "seq": 1},
                       {"cat": "nav", "kind": "lf_scan", "t": 6.572, "seq": 2, "ms": 572}])
        self.assertEqual(perf(events)["verdict"], "FAIL")

    def test_startup_known_shape_keeps_other_100ms_stage(self):
        events = perf_events({0: .8}, {0: "none"})
        slow_update(events, 0, 750, startup_and_setup_ms=573, grid_ms=150)
        events.extend([{"cat": "nav", "kind": "load_folder_begin", "t": 6, "seq": 7},
                       {"cat": "nav", "kind": "lf_scan", "t": 6.572, "seq": 7, "ms": 572},
                       {"cat": "nav", "kind": "load_folder_end", "t": 6.75, "seq": 7}])
        result = perf(events)
        self.assertEqual(result["verdict"], "FAIL")
        self.assertEqual(result["evidence"]["slow_updates"][0]["categories"], ["known-startup-scan", "unexplained"])

    def test_known_ai_keeps_extra_outer_update_work(self):
        events, evidence = self.ai_shape()
        evidence["first_ai"]["end_t"] = 6.9
        next(e for e in events if e.get("kind") == "first_ai")["end_t"] = 6.9
        next(e for e in events if e.get("cat") == "frame" and e.get("n") == 2).update(
            prev_update_ms=600, prev_update_end_t=6.6)
        result = perf(events, evidence)
        self.assertEqual(result["verdict"], "FAIL")
        self.assertIn({"stage": "outside_inner_update_frame", "ms": 180},
                      result["evidence"]["slow_updates"][0]["other_slow_stages"])

    def ai_shape(self, repeated=False):
        events = perf_events({0: .5, **({1: .5} if repeated else {})},
                             {0: "request_repaint_after_ai_upscale", **({1: "request_repaint_after_ai_upscale"} if repeated else {})})
        slow_update(events, 0, 420, fullscreen_viewport_ms=410)
        if repeated:
            slow_update(events, 1, 420, fullscreen_viewport_ms=410)
        evidence = operations()
        evidence["first_ai"] = {"included": True, "first": True, "started": True,
                                "start_t": 6, "end_t": 6.45, "model": "fixture-model",
                                "backend": "fixture-gpu", "pack": "fixture-pack"}
        events.append({"cat": "qa", "kind": "first_ai", "case_id": "perf-cached", "t": 6.45,
                       **evidence["first_ai"]})
        return events, evidence

    def test_early_return_ai_observed_delay_requires_proof_majority_and_overlap(self):
        events, evidence = self.ai_shape()
        evidence["first_ai"]["start_t"] = 6.01
        next(e for e in events if e.get("kind") == "first_ai")["start_t"] = 6.01
        next(e for e in events if e.get("kind") == "tail_repaint" and e.get("n") == 1).update(
            action="request_repaint_after_observed", early_return=True, delay_ms=16)
        result = perf(events, evidence)
        self.assertEqual(result["verdict"], "PASS_WITH_WARNINGS", result)
        self.assertEqual(result["metrics"]["update_categories"], {"known-first-ai": 1})
        for delay in (None, 0, -1):
            next(e for e in events if e.get("kind") == "tail_repaint" and e.get("n") == 1)["delay_ms"] = delay
            self.assertEqual(perf(events, evidence)["verdict"], "FAIL")
        next(e for e in events if e.get("kind") == "tail_repaint" and e.get("n") == 1)["delay_ms"] = 16
        slow_update(events, 0, 420, fullscreen_viewport_ms=200)
        self.assertEqual(perf(events, evidence)["verdict"], "FAIL")
        slow_update(events, 0, 420, fullscreen_viewport_ms=410)
        evidence["first_ai"]["start_t"] = 6.43
        next(e for e in events if e.get("kind") == "first_ai")["start_t"] = 6.43
        self.assertEqual(perf(events, evidence)["verdict"], "FAIL")
        evidence["first_ai"]["start_t"] = 6.01
        events = [e for e in events if e.get("kind") != "first_ai"]
        self.assertEqual(perf(events, evidence)["verdict"], "FAIL")

    def test_generic_observed_delay_is_not_normal_sleep(self):
        events = perf_events({0: .501}, {0: "request_repaint_after_observed"})
        next(e for e in events if e.get("kind") == "tail_repaint" and e.get("n") == 1).update(
            early_return=True, delay_ms=500)
        self.assertEqual(perf(events)["verdict"], "FAIL")

    def test_initial_ai_warning_requires_explicit_first_operation(self):
        events, evidence = self.ai_shape()
        result = perf(events, evidence)
        self.assertEqual(result["verdict"], "PASS_WITH_WARNINGS")
        self.assertEqual(result["metrics"]["update_categories"], {"known-first-ai": 1})
        evidence["first_ai"]["first"] = False
        self.assertEqual(perf(events, evidence)["verdict"], "FAIL")

    def test_repeated_ai_stop_is_not_initial_ai(self):
        events, evidence = self.ai_shape(True)
        result = perf(events, evidence)
        self.assertEqual(result["verdict"], "FAIL")
        self.assertEqual(result["metrics"]["update_categories"], {"known-first-ai": 1, "unexplained": 1})

    def test_request_repaint_unknown_gap_fails_once_per_frame(self):
        events = perf_events({0: .25})
        slow_update(events, 0, 200, grid_ms=180)
        result = perf(events)
        self.assertEqual(result["verdict"], "FAIL")
        self.assertEqual(result["metrics"]["unexplained_count"], 1)
        self.assertEqual(result["metrics"]["gap_count"], 1)
        self.assertEqual(result["metrics"]["slow_update_count"], 1)

    def test_unknown_delayed_wake_fails(self):
        self.assertEqual(perf(perf_events({0: 1}, {0: "request_repaint_after_unknown"}))["verdict"], "FAIL")

    def test_last_update_uses_inner_measurement_and_reports_outer_absence(self):
        events = perf_events()
        slow_update(events, 201, 200, grid_ms=180)
        result = perf(events)
        self.assertEqual(result["verdict"], "FAIL")
        self.assertFalse(result["evidence"]["slow_updates"][0]["outer_measured"])

    def test_100ms_boundary_is_not_above_threshold(self):
        result = perf(perf_events({0: .1}))
        self.assertEqual(result["verdict"], "PASS")
        self.assertEqual(result["metrics"]["gap_count"], 0)

    def test_exact_16ms_is_excluded_and_ratio_is_warning_only(self):
        result = perf(perf_events({n: .016 for n in range(7)}))
        self.assertEqual(result["verdict"], "PASS_WITH_WARNINGS")
        self.assertEqual(result["metrics"]["drawn"]["below_16ms"], 194)

    def test_p95_raw_boundary_keeps_startup_samples(self):
        self.assertEqual(perf(perf_events(pre_grid_ms=.3))["verdict"], "PASS")
        self.assertEqual(perf(perf_events(pre_grid_ms=.3000000000001))["verdict"], "FAIL")
        self.assertEqual(perf(perf_events(pre_grid_ms=.300001))["verdict"], "FAIL")
        events = perf_events()
        samples = [e for e in events if e.get("kind") == "pre_grid_breakdown"]
        for e in samples[:12]:
            e["total_ms"] = 50
        self.assertEqual(perf(events)["verdict"], "FAIL")

    def test_duplicate_pre_grid_samples_cannot_dilute_p95_into_pass(self):
        events = perf_events(pre_grid_ms=.4)
        duplicate = next(e for e in events if e.get("kind") == "pre_grid_breakdown")
        events.extend([{**duplicate, "total_ms": .2} for _ in range(5000)])
        result = perf(events)
        self.assertEqual(result["verdict"], "SKIPPED")
        self.assertEqual(result["metrics"]["pre_grid"]["p95_ms"], .2)
        self.assertTrue(any("duplicate pre_grid" in error for error in result["environment_errors"]))

    def test_missing_tail_breakdown_or_samples_never_pass(self):
        events = perf_events()
        events = [e for e in events if not (e.get("kind") == "tail_repaint" and e["n"] == 1)]
        self.assertEqual(perf(events)["verdict"], "SKIPPED")
        self.assertEqual(perf([e for e in perf_events() if e.get("kind") != "pre_grid_breakdown"])["verdict"], "SKIPPED")
        self.assertEqual(perf([session()])["verdict"], "SKIPPED")

    def test_lost_complete_frame_cannot_hide_in_normal_sleep(self):
        events = perf_events({99: 1}, {99: "none", 100: "none"}, frame_count=203)
        events = [e for e in events if e.get("n") != 101]
        result = perf(events)
        self.assertEqual(result["verdict"], "SKIPPED")
        self.assertEqual(result["metrics"]["drawn"]["n"], 200)
        self.assertTrue(any("nonconsecutive" in error for error in result["environment_errors"]))
        self.assertEqual(result["evidence"]["gaps"][0]["categories"], ["incomplete-evidence"])

    def test_missing_leading_frame_cannot_pass_with_enough_remaining_drawn_samples(self):
        events = [e for e in perf_events() if e.get("n") != 1]
        result = perf(events)
        self.assertEqual(result["verdict"], "SKIPPED")
        self.assertEqual(result["metrics"]["drawn"]["n"], 200)
        self.assertTrue(any("first frame n=1" in error for error in result["environment_errors"]))

    def test_mismatch_dropped_or_missing_operations_never_pass(self):
        events = perf_events()
        events[0]["pid"] = 99
        self.assertEqual(perf(events)["verdict"], "SKIPPED")
        events[0]["pid"] = PID
        events.append({"t": 7, "cat": "log", "kind": "diagnostic_dropped", "count": 1})
        self.assertEqual(perf(events)["verdict"], "SKIPPED")
        self.assertEqual(perf(perf_events(), {})["verdict"], "SKIPPED")

    def test_search_physical_paths_normalize_without_changing_owner_keys(self):
        evidence = operations()
        evidence["search"]["expected_hits"] = ["C:/fixture/Search/a.PNG", "//server/share/b.PNG"]
        evidence["search"]["actual_hits"] = [r"\\?\c:\fixture\search\A.png", r"\\?\UNC\SERVER\SHARE\B.png"]
        self.assertEqual(perf(perf_events(), evidence)["verdict"], "PASS")
        evidence["navigation"][0]["before_path"] = "C:/fixture/a"
        evidence["navigation"][0]["after_path"] = r"\\?\C:\fixture\A"
        self.assertEqual(perf(perf_events(), evidence)["verdict"], "SKIPPED")
        evidence = operations()
        evidence["cache"]["opens"][0]["target_keys"] = ["dir::FIXTURE/child"]
        self.assertEqual(perf(perf_events(), evidence)["verdict"], "SKIPPED")

    def test_recipe_receipt_or_repeat_does_not_prove_exact_chord_delivery(self):
        events = [e for e in perf_events() if e.get("kind") != "key_materialized"]
        self.assertEqual(perf(events)["verdict"], "SKIPPED")
        for field, value in (("ctrl", False), ("repeat", True), ("target_hwnd", 999), ("target_viewport", "OTHER")):
            with self.subTest(field=field):
                events = perf_events()
                next(e for e in events if e.get("kind") == "key_materialized")[field] = value
                self.assertEqual(perf(events)["verdict"], "SKIPPED")

    def test_uncached_requires_initial_source_and_pre_open_absence_not_later_no_cache_rule(self):
        events, evidence = perf_events(), operations()
        evidence["cache"]["mode"] = "uncached"
        for opened in evidence["cache"]["opens"]:
            opened["pre_open_absent"] = True
        for index, e in enumerate([e for e in events if e.get("kind") == "cache_read"]):
            e.update(kind="source", from_cache=False, request_id=10 + index)
            events.append({"cat": "qa", "kind": "request_begin", "t": 5.55 + index * .001,
                           "request_id": 10 + index})
        for e in events:
            if e.get("kind") == "on_exit":
                e["case_id"] = "perf-uncached"
        events.extend([{"cat": "qa", "kind": "request_begin", "t": 5.58, "request_id": 20},
                       {"cat": "qa", "kind": "cache_read", "t": 5.59, "request_id": 20,
                        "key": "dir::fixture/child", "items_gen": 3, "from_cache": True}])
        result = analyze_perf_smoke(events, expected_pid=PID, case_id="perf-uncached", evidence=evidence)
        self.assertEqual(result["verdict"], "PASS")
        evidence["cache"]["opens"][0]["pre_open_absent"] = False
        self.assertEqual(analyze_perf_smoke(events, expected_pid=PID, case_id="perf-uncached", evidence=evidence)["verdict"], "SKIPPED")

    def test_optional_ai_session_has_independent_proof_and_reports_drawn_samples(self):
        events, ai_evidence = self.ai_shape()
        events = [e for e in events if e.get("kind") != "first_ai"]
        evidence = {"pid": PID, "first_ai": ai_evidence["first_ai"], "cache": operations()["cache"],
                    "exit": operations()["exit"]}
        for e in events:
            if e.get("kind") == "on_exit":
                e["case_id"] = "first-ai"
        events.append({"cat": "qa", "kind": "first_ai", "case_id": "first-ai", "t": 6.45,
                       **evidence["first_ai"]})
        result = analyze_perf_smoke(events, expected_pid=PID, case_id="first-ai", evidence=evidence)
        self.assertEqual(result["verdict"], "PASS_WITH_WARNINGS", result)
        self.assertIsNone(result["metrics"]["drawn"]["minimum_samples"])
        sparse = [e for e in events if e.get("n", 0) <= 2]
        self.assertEqual(analyze_perf_smoke(sparse, expected_pid=PID, case_id="first-ai", evidence=evidence)["verdict"], "PASS_WITH_WARNINGS")
        events = [e for e in events if e.get("kind") != "first_ai"]
        self.assertEqual(analyze_perf_smoke(events, expected_pid=PID, case_id="first-ai", evidence=evidence)["verdict"], "FAIL")

    def test_partial_or_wrong_folder_cache_proof_cannot_pass_perf_case(self):
        evidence = operations()
        evidence["cache"]["opens"] = evidence["cache"]["opens"][1:]
        self.assertEqual(perf(perf_events(), evidence)["verdict"], "SKIPPED")
        evidence = operations()
        evidence["cache"]["opens"][0]["folder_path"] = "unrelated-folder"
        self.assertEqual(perf(perf_events(), evidence)["verdict"], "SKIPPED")
        evidence = operations()
        evidence["cache"]["opens"][4]["items_gen"] += 1
        self.assertEqual(perf(perf_events(), evidence)["verdict"], "SKIPPED")


class ClockTests(unittest.TestCase):
    def sampler(self):
        return {"expected_pid": PID, "measure_start_unix_ms": OS_START + 17_000,
                "measure_end_unix_ms": OS_START + 32_000, "wall_seconds": 15,
                "sleep_start_unix_ms": OS_START + 16_000, "sleep_end_unix_ms": OS_START + 33_000}

    def test_nonzero_creation_offset_and_wall_t_window(self):
        result = convert_clock_window([session()], self.sampler(), PID)
        self.assertEqual(result["verdict"], "PASS")
        clock = result["evidence"]["clock"]
        self.assertEqual((clock["start_t"], clock["end_t"]), (10, 25))
        events = [session(), {"t": 8, "cat": "ui", "kind": "tail_repaint", "n": 0, "action": "none"},
                  {"t": 11, "cat": "thumb", "kind": "enqueue", "key": "work-a"},
                  {"t": 24, "cat": "thumb", "kind": "enqueue", "key": "work-b"},
                  {"t": 26, "cat": "input", "kind": "exit"}]
        idle = analyze_idle_health(events, clock["start_t"], clock["end_t"], expected_pid=PID, allow_sleeping_window=True)
        self.assertEqual(idle["metrics"]["input_events"], 0)
        self.assertEqual(idle["metrics"]["events"], 2)
        wrong = analyze_idle_health(events, 17, 32, expected_pid=PID, allow_sleeping_window=True)
        self.assertEqual(wrong["metrics"]["input_events"], 1)

    def test_missing_anchor_no_fallback_to_os_creation_or_session_t(self):
        event = session()
        del event["wall_t"]
        self.assertEqual(convert_clock_window([event], self.sampler(), PID)["verdict"], "SKIPPED")

    def test_noncontainment_clock_change_reversal_or_mismatch_rejected(self):
        for field, value in (("sleep_start_unix_ms", OS_START + 18_000),
                             ("wall_seconds", 14), ("measure_end_unix_ms", OS_START + 16_000)):
            with self.subTest(field=field):
                sampler = self.sampler()
                sampler[field] = value
                self.assertEqual(convert_clock_window([session()], sampler, PID)["verdict"], "SKIPPED")
        self.assertEqual(convert_clock_window([session()], self.sampler(), 99)["verdict"], "SKIPPED")


class EvidenceTests(unittest.TestCase):
    def tray(self, completed=False):
        case = "tray-close-after-loading" if completed else "tray-close-while-loading"
        target = {"key": "dir::fixture/child", "idx": 7, "items_gen": 3,
                  "catalog_absent": True, "cache_map_absent": True}
        common = {"cat": "qa", "case_id": case, "close_sequence": 1, "items_gen": 3,
                  "folder_path": "fixture", "loading_complete": completed, "targets": [target]}
        req = {"key": target["key"], "idx": 7, "items_gen": 3, "request_id": 10, "input_seq": 1}
        events = [session(), {**common, "kind": "close_accept", "t": 8},
                  {**common, "kind": "tray_hide", "t": 8.1},
                  {"cat": "qa", "kind": "request_begin", "case_id": case, "t": 8.2, **req},
                  {"cat": "qa", "kind": "source", "case_id": case, "t": 8.3, **req,
                   "should_save": False, "from_cache": False},
                  {"cat": "qa", "kind": "finalized", "case_id": case, "t": 8.4, **req,
                   "close_sequence": 1, "image_backlog_contains_request": True, "window_visible": False},
                  {"cat": "qa", "kind": "on_exit", "case_id": case, "t": 34, "flush_complete": True}]
        evidence = {"case_id": case, "pid": PID, "exit": {"process_exited": True, "forced": False}}
        return events, evidence

    def test_both_tray_cases_independently_require_complete_proof(self):
        for completed in (False, True):
            events, evidence = self.tray(completed)
            self.assertEqual(analyze_case_evidence(events, evidence, PID, setup_deadline_t=10)["verdict"], "PASS")
            events[1]["loading_complete"] = not completed
            self.assertEqual(analyze_case_evidence(events, evidence, PID, setup_deadline_t=10)["verdict"], "SKIPPED")

    def test_cache_hit_pre_hide_source_save_or_no_backlog_is_not_reproduction(self):
        for kind, field, value in (("source", "from_cache", True), ("request_begin", "t", 7),
                                   ("source", "should_save", True),
                                   ("finalized", "image_backlog_contains_request", False),
                                   ("finalized", "request_id", 11)):
            with self.subTest(field=field):
                events, evidence = self.tray()
                next(e for e in events if e["kind"] == kind)[field] = value
                self.assertEqual(analyze_case_evidence(events, evidence, PID, setup_deadline_t=10)["verdict"], "SKIPPED")

    def test_same_seq_does_not_prove_same_request(self):
        events, evidence = self.tray()
        for e in events:
            e.pop("request_id", None)
        self.assertEqual(analyze_case_evidence(events, evidence, PID, setup_deadline_t=10)["verdict"], "SKIPPED")

    def test_seed_recipe_or_script_exit_zero_does_not_prove_normal_exit(self):
        events, evidence = self.tray()
        evidence.update(seed_kind="video-pin", target_key="dir::fixture/child", copied_after_process_exit=True)
        self.assertEqual(analyze_case_evidence(events, evidence, PID, setup_deadline_t=10)["verdict"], "SKIPPED")
        evidence["exit"]["forced"] = True
        self.assertEqual(analyze_case_evidence(events, evidence, PID, setup_deadline_t=10)["verdict"], "SKIPPED")

    def test_standalone_tray_requires_measurement_start_deadline(self):
        for completed in (False, True):
            events, evidence = self.tray(completed)
            for deadline in (None, -1, float("nan"), True):
                with self.subTest(completed=completed, deadline=deadline):
                    result = analyze_case_evidence(events, evidence, PID, setup_deadline_t=deadline)
                    self.assertEqual(result["verdict"], "SKIPPED")
                    self.assertIn("tray converted measurement-start setup deadline missing/invalid",
                                  result["environment_errors"])

    def test_every_tray_setup_witness_must_exist_by_start_including_save_decision(self):
        for completed in (False, True):
            for kind in ("close_accept", "tray_hide", "request_begin", "source", "finalized"):
                with self.subTest(completed=completed, kind=kind):
                    events, evidence = self.tray(completed)
                    next(e for e in events if e.get("kind") == kind)["t"] = math.nextafter(10, math.inf)
                    result = analyze_case_evidence(events, evidence, PID, setup_deadline_t=10)
                    self.assertEqual(result["verdict"], "SKIPPED")
            events, evidence = self.tray(completed)
            source = next(e for e in events if e.get("kind") == "source")
            source.pop("should_save")
            next(e for e in events if e.get("kind") == "finalized")["t"] = 10
            load = {**source, "cat": "thumb", "kind": "load_phases", "t": 10,
                    "total_ms": 50, "should_save": False, "force_cache": False}
            events.append(load)
            self.assertEqual(analyze_case_evidence(events, evidence, PID, setup_deadline_t=10)["verdict"], "PASS")
            load["t"] = math.nextafter(10, math.inf)
            self.assertEqual(analyze_case_evidence(events, evidence, PID, setup_deadline_t=10)["verdict"], "SKIPPED")

    def test_combined_seed_preparation_does_not_invent_cache_reread(self):
        case = "seed-profile"
        events = [session(), {"cat": "qa", "kind": "seed", "case_id": case, "t": 6,
                              "seed_kind": "cache", "normal_producer": True, "saved_cache": True,
                              "targets": [f"dir::{Path('fixture/child/pin.mp4').parent}"]},
                  {"cat": "qa", "kind": "seed", "case_id": case, "t": 7,
                   "seed_kind": "video-pin", "normal_producer": True, "pin_saved": True,
                   "key": "fixture/child/pin.mp4"},
                  {"cat": "qa", "kind": "seed", "case_id": case, "t": 8,
                   "seed_kind": "search", "normal_producer": True},
                  {"cat": "index", "kind": "initial_scan_settled", "t": 9},
                  {"cat": "qa", "kind": "on_exit", "case_id": case, "t": 10, "flush_complete": True}]
        evidence = {"case_id": case, "pid": PID, "seed_phase": "preparation",
                    "seed_kinds": ["cache", "video-pin", "search"], "copied_after_process_exit": True,
                    "search": {"completed": True, "expected_hits": ["a"], "actual_hits": ["a"]},
                    "exit": {"process_exited": True, "forced": False}}
        self.assertEqual(analyze_case_evidence(events, evidence, PID)["verdict"], "PASS")
        evidence["seed_phase"] = "reload"
        self.assertEqual(analyze_case_evidence(events, evidence, PID)["verdict"], "SKIPPED")


class IdleTests(unittest.TestCase):
    def setup_case(self):
        case = "static-foreground"
        events = [session(), {"cat": "frame", "kind": "begin", "t": 8, "n": 1},
                  {"cat": "ui", "kind": "tail_repaint", "t": 8, "n": 1, "action": "none"},
                  {"cat": "qa", "kind": "cache_read", "t": 8, "key": "dir::fixture/child",
                   "items_gen": 3, "from_cache": True},
                  {"cat": "qa", "kind": "request_begin", "t": 7.9, "key": "dir::fixture/child",
                   "items_gen": 3, "evaluated_display_px": 374},
                  {"cat": "qa", "kind": "on_exit", "case_id": case, "t": 34, "flush_complete": True}]
        sampler = ClockTests().sampler()
        sampler.update(case_id=case, cpu_core_ratio=.01, app_log_growth_bytes=0, perf_log_growth_bytes=0,
                       index_scan_wait="settled", identity_evidence="command_line", portable_identity_verified=True,
                       process_alive_start=True, process_alive_end=True, root_hwnd=123,
                       window_samples=[{"unix_ms": OS_START + n * 1000, "foreground_pid": PID,
                                        "foreground_hwnd": 123, "total": 1, "visible": 1} for n in range(17, 33)])
        evidence = {"case_id": case, "pid": PID, "exit": {"process_exited": True, "forced": False},
                    "cache": operations()["cache"], "requested_px": 374}
        thresholds = json.loads(Path(__file__).with_name("idle_health_thresholds.json").read_text(encoding="utf-8"))
        return events, sampler, evidence, thresholds

    def test_sleeping_window_requires_external_identity_and_full_foreground_lifetime(self):
        events, sampler, evidence, thresholds = self.setup_case()
        self.assertEqual(analyze_idle_case(events, sampler, evidence, thresholds, PID)["verdict"], "PASS_WITH_WARNINGS")
        sampler["window_samples"][7]["foreground_pid"] = 99
        self.assertEqual(analyze_idle_case(events, sampler, evidence, thresholds, PID)["verdict"], "SKIPPED")

    def test_tray_reproduction_complete_by_converted_start_not_measurement_or_sleep_end(self):
        for completed in (False, True):
            for finalized_t, expected in ((8.4, "PASS_WITH_WARNINGS"), (10, "PASS"),
                                          (math.nextafter(10, math.inf), "SKIPPED"),
                                          (12, "SKIPPED"), (31.4, "SKIPPED")):
                with self.subTest(completed=completed, finalized_t=finalized_t):
                    _, sampler, _, thresholds = self.setup_case()
                    events, evidence = EvidenceTests().tray(completed)
                    sampler["case_id"] = evidence["case_id"]
                    # Keep all late setup events within sleep: this is not sufficient.
                    sampler["sleep_end_unix_ms"] = OS_START + 40_000
                    for sample in sampler["window_samples"]:
                        sample["visible"] = 0
                    events.extend([{"cat": "frame", "kind": "begin", "t": 7.9, "n": 1},
                                   {"cat": "ui", "kind": "tail_repaint", "t": 7.9, "n": 1, "action": "none"}])
                    next(e for e in events if e.get("kind") == "request_begin")["t"] = finalized_t - .2
                    next(e for e in events if e.get("kind") == "source")["t"] = finalized_t - .1
                    next(e for e in events if e.get("kind") == "finalized")["t"] = finalized_t
                    result = analyze_idle_case(events, sampler, evidence, thresholds, PID)
                    self.assertEqual(result["verdict"], expected, result)
                    clock = result["evidence"]["clock"]
                    self.assertEqual((clock["start_t"], clock["end_t"]), (10, 25))
                    self.assertEqual(result["evidence"]["tray"]["setup_deadline_t"], 10)
                    targets = result["evidence"]["tray"]["valid_targets"]
                    if expected in ("PASS", "PASS_WITH_WARNINGS"):
                        self.assertEqual(targets[0]["finalized_t"], finalized_t)
                        self.assertLessEqual(targets[0]["source_t"], 10)
                    else:
                        self.assertEqual(targets, [])

    def test_cpu_and_log_limits_fail_without_threshold_relaxation(self):
        for field, value in (("cpu_core_ratio", .10001), ("app_log_growth_bytes", 16385),
                             ("perf_log_growth_bytes", 262145)):
            with self.subTest(field=field):
                events, sampler, evidence, thresholds = self.setup_case()
                sampler[field] = value
                self.assertEqual(analyze_idle_case(events, sampler, evidence, thresholds, PID)["verdict"], "FAIL")

    def test_unsettled_index_or_missing_lifetime_is_environment_incomplete(self):
        for field, value in (("index_scan_wait", "timeout"), ("portable_identity_verified", False),
                             ("process_alive_end", False), ("window_samples", [])):
            with self.subTest(field=field):
                events, sampler, evidence, thresholds = self.setup_case()
                sampler[field] = value
                self.assertEqual(analyze_idle_case(events, sampler, evidence, thresholds, PID)["verdict"], "SKIPPED")

    def test_cpu_target_warning_and_sampler_warnings_survive_merge(self):
        events, sampler, evidence, thresholds = self.setup_case()
        sampler.update(cpu_core_ratio=.06, verdict="PASS_WITH_WARNINGS", warnings=["external sampler witness warning"])
        result = analyze_idle_case(events, sampler, evidence, thresholds, PID)
        self.assertEqual(result["verdict"], "PASS_WITH_WARNINGS")
        self.assertIn("external sampler witness warning", result["warnings"])
        self.assertTrue(any("CPU one-core" in w for w in result["warnings"]))

    def test_foreground_hwnd_and_owned_background_pid_are_exact(self):
        events, sampler, evidence, thresholds = self.setup_case()
        sampler["window_samples"][6]["foreground_hwnd"] = 999
        self.assertEqual(analyze_idle_case(events, sampler, evidence, thresholds, PID)["verdict"], "SKIPPED")
        events, sampler, evidence, thresholds = self.setup_case()
        evidence["case_id"] = sampler["case_id"] = "static-background"
        next(e for e in events if e.get("kind") == "on_exit")["case_id"] = "static-background"
        sampler.update(background_hwnd=456, background_process_id=99)
        for sample in sampler["window_samples"]:
            sample.update(foreground_pid=99, foreground_hwnd=456)
        self.assertEqual(analyze_idle_case(events, sampler, evidence, thresholds, PID)["verdict"], "PASS_WITH_WARNINGS")
        sampler["window_samples"][6]["foreground_pid"] = 100
        self.assertEqual(analyze_idle_case(events, sampler, evidence, thresholds, PID)["verdict"], "SKIPPED")

    def test_static_requested_pixel_boundaries_and_real_worker_match(self):
        for pixels, expected in ((366, "PASS_WITH_WARNINGS"), (382, "PASS_WITH_WARNINGS"),
                                 (365, "SKIPPED"), (383, "SKIPPED")):
            with self.subTest(pixels=pixels):
                events, sampler, evidence, thresholds = self.setup_case()
                evidence["requested_px"] = pixels
                next(e for e in events if e.get("kind") == "request_begin")["evaluated_display_px"] = pixels
                result = analyze_idle_case(events, sampler, evidence, thresholds, PID)
                self.assertEqual(result["verdict"], expected)
                self.assertEqual(result["metrics"]["requested_px"], pixels)
        events, sampler, evidence, thresholds = self.setup_case()
        next(e for e in events if e.get("kind") == "request_begin")["items_gen"] = 4
        self.assertEqual(analyze_idle_case(events, sampler, evidence, thresholds, PID)["verdict"], "SKIPPED")


class CliTests(unittest.TestCase):
    def test_missing_and_malformed_log_retained(self):
        with test_directory() as path:
            self.assertTrue(load_events_strict(path / "absent")[1])
            log = path / "perf.jsonl"
            log.write_text(json.dumps(session()) + '\n{"t":NaN}\nnot-json\n[]\n', encoding="utf-8")
            events, errors = load_events_strict(log)
            self.assertEqual(len(events), 1)
            self.assertEqual(len(errors), 3)

    def test_existing_cli_dispatch_uses_strict_loader_and_missing_exit_two(self):
        with test_directory() as path:
            result = subprocess.run([sys.executable, str(Path(__file__).with_name("analyze_perf.py")),
                                     str(path / "missing.jsonl"), "perf-smoke", "--expected-pid", str(PID),
                                     "--json-out", str(path / "case.json")], capture_output=True, text=True)
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertEqual(json.loads((path / "case.json").read_text(encoding="utf-8"))["verdict"], "SKIPPED")

    def test_suite_fail_has_precedence_over_missing_or_skipped(self):
        cases = [finish(report(case_id)) for case_id in REQUIRED_CASES]
        self.assertEqual(summarize_suite(cases, metadata())["verdict"], "PASS")
        cases[0]["environment_errors"].append("missing witness")
        finish(cases[0])
        self.assertEqual(summarize_suite(cases, metadata())["exit_code"], 2)
        cases[1]["failures"].append("product regression")
        finish(cases[1])
        result = summarize_suite(cases[:-1], metadata())
        self.assertEqual(result["exit_code"], 1)
        self.assertTrue(result["environment_errors"])
        self.assertIn("実データ", release_record(result))

    def test_summary_json_and_markdown_are_artifacts_without_history_write(self):
        with test_directory() as path:
            inputs = path / "suite-input.json"
            inputs.write_text(json.dumps({"metadata": metadata(),
                                         "cases": [finish(report(c)) for c in REQUIRED_CASES]}), encoding="utf-8")
            with contextlib.redirect_stdout(io.StringIO()):
                code = main(["summary", str(inputs), "--json-out", str(path / "summary.json"),
                             "--markdown-out", str(path / "release-record.md")])
            self.assertEqual(code, 0)
            self.assertEqual(json.loads((path / "summary.json").read_text(encoding="utf-8"))["schema_version"], 1)
            self.assertIn("synthetic", (path / "release-record.md").read_text(encoding="utf-8"))

    def test_metadata_missing_or_requested_ai_absent_prohibits_suite_pass(self):
        cases = [finish(report(c)) for c in REQUIRED_CASES]
        self.assertEqual(summarize_suite(cases)["verdict"], "SKIPPED")
        meta = metadata()
        meta["include_first_ai"] = True
        self.assertEqual(summarize_suite(cases, meta)["verdict"], "SKIPPED")

    def test_missing_or_malformed_case_artifact_preserves_other_case_failure(self):
        with test_directory() as path:
            product = report("perf-cached")
            product["failures"].append("real perf failure")
            finish(product)
            failed = path / "failed.json"
            failed.write_text(json.dumps(product), encoding="utf-8")
            for invalid in ("missing.json", 7):
                with self.subTest(invalid=invalid):
                    inputs = path / "suite.json"
                    inputs.write_text(json.dumps({"metadata": metadata(), "cases": [str(failed), invalid]}), encoding="utf-8")
                    with contextlib.redirect_stdout(io.StringIO()):
                        code = main(["summary", str(inputs), "--json-out", str(path / "summary.json"),
                                     "--markdown-out", str(path / "record.md")])
                    result = json.loads((path / "summary.json").read_text(encoding="utf-8"))
                    self.assertEqual(code, 1)
                    self.assertEqual(result["verdict"], "FAIL")
                    self.assertTrue(result["environment_errors"])
                    self.assertIn("real perf failure", (path / "record.md").read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
