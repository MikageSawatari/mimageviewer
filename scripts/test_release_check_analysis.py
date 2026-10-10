#!/usr/bin/env python3
"""Synthetic release-check and Phase 3b regressions. No App or desktop interaction."""
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
            "session_id": "synthetic-session", "log_delivery_schema": 1,
            "process_start_unix_ms": OS_START}


def seal(events: list[dict]) -> list[dict]:
    """Finish a synthetic saved log with the real shutdown delivery schema."""
    events = [e for e in events if e.get("kind") != "drain_checkpoint"]
    coverage = max(e["t"] for e in events)
    events.append({"cat": "log", "kind": "drain_checkpoint", "t": coverage + .1,
                   "schema": 1, "session_id": "synthetic-session", "pid": PID,
                   "process_start_unix_ms": OS_START, "request_id": "synthetic-shutdown", "purpose": "shutdown",
                   "coverage_t": coverage, "ticket_cutoff": 100, "settled_watermark": 100, "written_watermark": 100,
                   "normal_status": "ready", "perf_status": "ready", "normal_dropped_total": 0,
                   "perf_dropped_total": 0, "diagnostic_dropped_total": 0,
                   "normal_written_bytes": 1000, "perf_written_bytes": 1000})
    return events


def cached_request(key: str, generation: int, request_id: int, *, t: float = 5.6, idx: int = 0) -> list[dict]:
    identity = {"key": key, "items_gen": generation, "request_id": request_id, "idx": idx}
    return [{"cat": "qa", "kind": "request_begin", "t": t - .01, "evaluated_display_px": 374, **identity},
            {"cat": "qa", "kind": "cache_read", "t": t, "from_cache": True,
             "origin": "upgradeable_cache", **identity}]


def folder_loads() -> list[dict]:
    return [{"before_path": "fixture" if i == 0 else f"fixture/folder-{i:04d}",
             "after_path": f"fixture/folder-{i + 1:04d}", "before_generation": 3 + i, "after_generation": 4 + i,
             "start_t": 5.8 + i * .009, "end_t": 5.807 + i * .009} for i in range(256)]


def operations() -> dict:
    return {"pid": PID, "folder_open": {"completed": True},
            "navigation": [{"receipt": True, "before_generation": n + 260, "after_generation": n + 261,
                            "before_path": f"folder{n}", "after_path": f"folder{n + 1}",
                            "start_t": 8.3 + n * .02, "end_t": 8.31 + n * .02} for n in range(5)],
            "search": {"ctrl_g_receipt": True, "text_receipt": True, "completed": True,
                       "expected_hits": ["a", "b"], "actual_hits": ["b", "a"],
                       "start_t": 8.4, "end_t": 8.5, "expected_query": "fixture-query", "actual_query": "fixture-query"},
            "folder_loads": folder_loads(),
            "observation": {"root_hwnd": 123},
            "exit": {"on_exit": True, "flush_complete": True, "process_exited": True, "forced": False},
            "cache": {"mode": "cached", "opens": [{"folder_path": "fixture", "items_gen": 3,
                                                     "target_keys": ["dir::fixture/child"]},
                                                    *[{"folder_path": p["after_path"], "items_gen": p["after_generation"],
                                                       "target_keys": [p["after_path"] + "/image.png"]} for p in folder_loads()],
                                                    *[{"folder_path": f"folder{n}", "items_gen": n + 260,
                                                       "target_keys": [f"folder{n}/image.png"]} for n in range(6)]]},
            "fixture_manifest": {"perf": {mode: {"many_folders": "fixture", "start_folder": "folder0",
                                                     "load_order": [f"fixture/folder-{i:04d}" for i in range(1, 257)],
                                                     "navigation_order": [f"folder{n}" for n in range(6)]}
                                           for mode in ("cached", "uncached")}}}


def metadata() -> dict:
    return {"source": "synthetic", "exe_sha256": "synthetic", "fixture_version": "synthetic",
            "environment": "synthetic", "run_path": "synthetic", "approval_scope": "synthetic",
            "thresholds_version": "synthetic"}


def perf_events(gaps: dict[int, float] | None = None, actions: dict[int, str] | None = None,
                pre_grid_ms: float = .2, frame_count: int = 202) -> list[dict]:
    gaps, actions = gaps or {}, actions or {}
    events, t = [session(), *cached_request("dir::fixture/child", 3, 1)], 6.0
    for n in range(5):
        events.append({"cat": "qa", "kind": "key_materialized", "t": 8.305 + n * .02,
                       "key": "Down", "ctrl": True, "shift": False, "alt": False,
                       "pressed": True, "repeat": False, "target_viewport": "ROOT", "target_hwnd": 123})
    for n in range(6):
        events.extend(cached_request(f"folder{n}/image.png", n + 260, n + 2))
    events.extend([{"cat": "qa", "kind": "key_materialized", "t": 8.42,
                    "key": "G", "ctrl": True, "shift": False, "alt": False,
                    "pressed": True, "repeat": False, "target_viewport": "ROOT", "target_hwnd": 123},
                   {"cat": "qa", "kind": "text", "t": 8.43, "text": "fixture-query", "root_hwnd": 123}])
    for load in folder_loads():
        events.extend(cached_request(load["after_path"] + "/image.png", load["after_generation"],
                                     load["after_generation"] + 1000))
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
    events.append({"cat": "qa", "kind": "on_exit", "case_id": "perf-cached", "t": max(t + 1, 8.6),
                   "flush_complete": True})
    return seal(events)


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
    return analyze_perf_smoke(seal(events), expected_pid=PID, case_id="perf-cached",
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

    def test_workload_requires_distinct_folders_changed_generations_and_cache_proof(self):
        self.assertEqual(perf(perf_events())["verdict"], "PASS")
        for field, value in (("after_path", "wrong-folder"), ("after_generation", 3),
                             ("before_path", "wrong-folder"), ("start_t", 9)):
            with self.subTest(field=field):
                evidence = operations()
                evidence["folder_loads"][0][field] = value
                self.assertTrue(any("folder-load" in e for e in perf(perf_events(), evidence)["environment_errors"]))
        evidence = operations()
        evidence["folder_loads"].pop()
        self.assertEqual(perf(perf_events(), evidence)["verdict"], "SKIPPED")
        evidence = operations()
        evidence["fixture_manifest"]["perf"]["cached"]["load_order"][1] = "fixture/folder-0001"
        self.assertEqual(perf(perf_events(), evidence)["verdict"], "SKIPPED")
        events = [e for e in perf_events() if e.get("key") != "fixture/folder-0017/image.png"]
        self.assertEqual(perf(events)["verdict"], "SKIPPED")

    def test_first_frame_outside_update_stall_is_not_waived_by_folder_workload(self):
        result = perf(perf_events(gaps={0: .127}))
        self.assertEqual(result["verdict"], "FAIL")
        self.assertEqual(result["metrics"]["drawn"]["n"], 201)
        self.assertIn("frames [1]", result["failures"][0])

    def test_cached_parent_requires_all_245_runtime_targets_not_105_seed_targets(self):
        events, evidence = perf_events(), operations()
        keys = [f"dir::fixture/folder-{i:04d}" for i in range(1, 246)]
        evidence["cache"]["opens"][0]["target_keys"] = keys
        for i, key in enumerate(keys):
            rows = cached_request(key, 3, 12000 + i)
            if i >= 105:
                rows[1].update(kind="source", from_cache=False, origin="source_generated")
            events.extend(rows)
        result = perf(events, evidence)
        self.assertEqual(result["verdict"], "SKIPPED")
        self.assertTrue(any("cache/source provenance not proven" in e for e in result["environment_errors"]))
        # A later warm read cannot relabel initially generated data as cached.
        for i, key in enumerate(keys[105:]):
            events.extend(cached_request(key, 3, 13000 + i, t=5.8))
        self.assertEqual(perf(events, evidence)["verdict"], "SKIPPED")

    def test_uncached_requires_initial_source_and_pre_open_absence_not_later_no_cache_rule(self):
        events, evidence = perf_events(), operations()
        evidence["cache"]["mode"] = "uncached"
        for opened in evidence["cache"]["opens"]:
            opened["pre_open_absent"] = True
        for index, e in enumerate([e for e in events if e.get("kind") == "cache_read"]):
            e.update(kind="source", from_cache=False, origin="source_generated")
            next(b for b in events if b.get("kind") == "request_begin"
                 and b.get("request_id") == e["request_id"])["t"] = 5.55 + index * .00001
        for e in events:
            if e.get("kind") == "on_exit":
                e["case_id"] = "perf-uncached"
        events.extend(cached_request("dir::fixture/child", 3, 20, t=5.59))
        events = seal(events)
        result = analyze_perf_smoke(events, expected_pid=PID, case_id="perf-uncached", evidence=evidence)
        self.assertEqual(result["verdict"], "PASS")
        evidence["cache"]["opens"][0]["pre_open_absent"] = False
        self.assertEqual(analyze_perf_smoke(events, expected_pid=PID, case_id="perf-uncached", evidence=evidence)["verdict"], "SKIPPED")

    def test_uncached_provenance_requires_registered_begin_origin_and_earliest_request(self):
        def fixture():
            events, evidence = perf_events(), operations()
            evidence["cache"]["mode"] = "uncached"
            for opened in evidence["cache"]["opens"]:
                opened["pre_open_absent"] = True
            for event in events:
                if event.get("kind") == "cache_read":
                    event.update(kind="source", origin="source_generated", from_cache=False)
                elif event.get("kind") == "on_exit":
                    event["case_id"] = "perf-uncached"
            return events, evidence

        for kind, field, value in (("source", "request_id", 999), ("source", "origin", None),
                                   ("request_begin", "request_id", 0), ("request_begin", "idx", 7),
                                   ("request_begin", "t", 5.7)):
            with self.subTest(kind=kind, field=field):
                events, evidence = fixture()
                next(e for e in events if e.get("kind") == kind)[field] = value
                result = analyze_perf_smoke(events, expected_pid=PID, case_id="perf-uncached", evidence=evidence)
                self.assertEqual(result["verdict"], "SKIPPED")
        events, evidence = fixture()
        self.assertEqual(analyze_perf_smoke(events, expected_pid=PID, case_id="perf-uncached",
                                           evidence=evidence)["verdict"], "PASS")
        earlier_cached = cached_request("dir::fixture/child", 3, 100, t=5.57)
        result = analyze_perf_smoke(seal([*events, *earlier_cached]), expected_pid=PID,
                                    case_id="perf-uncached", evidence=evidence)
        self.assertEqual(result["verdict"], "SKIPPED")

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
        events = seal(events)
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
        next(o for o in evidence["cache"]["opens"] if o["folder_path"] == "folder3")["items_gen"] += 1
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
                  "folder_path": "fixture", "loading_complete": completed, "targets": [target],
                  "cache_check_unix_ms": OS_START + 14_900}
        req = {"key": target["key"], "idx": 7, "items_gen": 3, "request_id": 10, "input_seq": 1}
        events = [session(), {**common, "kind": "close_accept", "t": 8},
                  {**common, "kind": "tray_hide", "t": 8.1, "window_visible": False},
                  {"cat": "qa", "kind": "request_begin", "case_id": case, "t": 8.2,
                   "evaluated_display_px": 374, **req},
                  {"cat": "qa", "kind": "source", "case_id": case, "t": 8.3, **req,
                   "should_save": False, "force_cache": False, "from_cache": False, "origin": "source_generated"},
                  {"cat": "qa", "kind": "finalized", "case_id": case, "t": 8.4, **req,
                   "close_sequence": 1, "image_backlog_contains_request": True, "window_visible": False},
                  {"cat": "qa", "kind": "on_exit", "case_id": case, "t": 34, "flush_complete": True}]
        evidence = {"case_id": case, "pid": PID, "exit": {"process_exited": True, "forced": False}}
        return seal(events), evidence

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
            if e.get("cat") == "qa":
                e.pop("request_id", None)
        self.assertEqual(analyze_case_evidence(events, evidence, PID, setup_deadline_t=10)["verdict"], "SKIPPED")

    def test_tray_canonical_identity_origin_and_explicit_save_fields_are_required(self):
        for completed in (False, True):
            for kind, field, value in (
                ("request_begin", "request_id", True), ("request_begin", "request_id", 1 << 64),
                ("request_begin", "request_id", 0), ("request_begin", "request_id", "10"),
                ("request_begin", "evaluated_display_px", None), ("request_begin", "idx", 8),
                ("request_begin", "items_gen", 4), ("request_begin", "key", "other"),
                ("source", "request_id", 11), ("source", "origin", "upgradeable_cache"),
                ("source", "origin", "source_intrinsic"),
                ("source", "origin", None), ("source", "should_save", None),
                ("source", "force_cache", None), ("source", "force_cache", True),
                ("finalized", "request_id", 11), ("finalized", "idx", True),
                ("tray_hide", "window_visible", True),
            ):
                with self.subTest(completed=completed, kind=kind, field=field, value=value):
                    events, evidence = self.tray(completed)
                    next(e for e in events if e.get("kind") == kind)[field] = value
                    result = analyze_case_evidence(events, evidence, PID, setup_deadline_t=10)
                    self.assertEqual(result["verdict"], "SKIPPED")
                    self.assertEqual(result["evidence"].get("tray", {}).get("valid_targets", []), [])

    def test_tray_uint64_boundary_and_other_id_save_do_not_change_this_request(self):
        for completed in (False, True):
            events, evidence = self.tray(completed)
            for event in events:
                if event.get("cat") == "qa" and "request_id" in event:
                    event["request_id"] = (1 << 64) - 1
            other_source = {**next(e for e in events if e.get("kind") == "source"),
                            "request_id": 11, "should_save": True}
            other_begin = {**next(e for e in events if e.get("kind") == "request_begin"), "request_id": 11}
            events = seal([*events, other_begin, other_source])
            result = analyze_case_evidence(events, evidence, PID, setup_deadline_t=10)
            self.assertEqual(result["verdict"], "PASS", result)
            self.assertEqual(result["evidence"]["tray"]["valid_targets"][0]["request_id"], (1 << 64) - 1)

    def test_tray_boolean_sequence_and_target_generation_do_not_alias_uint_one(self):
        for completed in (False, True):
            for kind, field in (("close_accept", "close_sequence"), ("tray_hide", "close_sequence"),
                                ("finalized", "close_sequence"), ("tray_hide", "target_items_gen")):
                with self.subTest(completed=completed, kind=kind, field=field):
                    events, evidence = self.tray(completed)
                    for event in events:
                        if "items_gen" in event:
                            event["items_gen"] = 1
                        for target in event.get("targets", []):
                            target["items_gen"] = 1
                    events = json.loads(json.dumps(events))
                    self.assertEqual(analyze_case_evidence(events, evidence, PID,
                                                         setup_deadline_t=10)["verdict"], "PASS")
                    event = next(e for e in events if e.get("kind") == kind)
                    if field == "target_items_gen":
                        event["targets"][0]["items_gen"] = True
                    else:
                        event[field] = True
                    self.assertEqual(analyze_case_evidence(events, evidence, PID,
                                                         setup_deadline_t=10)["verdict"], "SKIPPED")

    def test_tray_lifecycle_order_not_jsonl_row_order_and_duplicate_begin_is_ambiguous(self):
        events, evidence = self.tray()
        # The writer need not order producers by timestamp, but shutdown proof is last.
        reordered = list(reversed(events[:-1])) + events[-1:]
        self.assertEqual(analyze_case_evidence(reordered, evidence, PID, setup_deadline_t=10)["verdict"], "PASS")
        for kind, timestamp in (("request_begin", 8.35), ("source", 8.45), ("finalized", 8.25)):
            with self.subTest(kind=kind):
                events, evidence = self.tray()
                next(e for e in events if e.get("kind") == kind)["t"] = timestamp
                self.assertEqual(analyze_case_evidence(events, evidence, PID, setup_deadline_t=10)["verdict"], "SKIPPED")
        events, evidence = self.tray()
        duplicate = dict(next(e for e in events if e.get("kind") == "request_begin"))
        self.assertEqual(analyze_case_evidence(seal([*events, duplicate]), evidence, PID,
                                             setup_deadline_t=10)["verdict"], "SKIPPED")

    def test_tray_cache_absence_clock_requires_same_anchor_and_pre_close_proof(self):
        events, evidence = self.tray()
        result = analyze_case_evidence(events, evidence, PID, setup_deadline_t=10)
        self.assertAlmostEqual(result["evidence"]["tray"]["cache_check_t"], 7.9)
        for field, value in (("cache_check_unix_ms", None), ("cache_check_unix_ms", OS_START + 15_100),
                             ("cache_check_unix_ms", OS_START + 17_001), ("cache_check_unix_ms", OS_START + 6_999)):
            with self.subTest(value=value):
                events, evidence = self.tray()
                for event in events:
                    if event.get("kind") in ("close_accept", "tray_hide"):
                        event[field] = value
                result = analyze_case_evidence(events, evidence, PID, setup_deadline_t=10)
                self.assertEqual(result["verdict"], "SKIPPED")
        events, evidence = self.tray()
        next(e for e in events if e.get("kind") == "tray_hide")["cache_check_unix_ms"] += 1
        self.assertEqual(analyze_case_evidence(events, evidence, PID, setup_deadline_t=10)["verdict"], "SKIPPED")
        events, evidence = self.tray()
        events[0].pop("wall_t")
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
            self.assertEqual(analyze_case_evidence(seal(events), evidence, PID, setup_deadline_t=10)["verdict"], "SKIPPED")
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
        self.assertEqual(analyze_case_evidence(seal(events), evidence, PID)["verdict"], "PASS")
        evidence["seed_phase"] = "reload"
        self.assertEqual(analyze_case_evidence(events, evidence, PID)["verdict"], "SKIPPED")


    def test_perf_seed_requires_durable_save_for_every_intended_folder(self):
        case = "seed-perf-cached"
        events = [session(), {"cat": "qa", "kind": "seed", "case_id": case, "t": 6,
                             "seed_kind": "cache", "normal_producer": True, "saved_cache": True,
                             "folder_path": "fixture/cached", "targets": ["dir::fixture/cached/child"]},
                  {"cat": "qa", "kind": "on_exit", "case_id": case, "t": 7, "flush_complete": True}]
        evidence = {"case_id": case, "pid": PID, "seed_phase": "preparation", "seed_kinds": ["cache"],
                    "seed_cache_paths": ["fixture/cached"], "copied_after_process_exit": True,
                    "exit": {"process_exited": True, "forced": False}}
        self.assertEqual(analyze_case_evidence(seal(events), evidence, PID)["verdict"], "PASS")
        evidence["seed_cache_paths"].append("fixture/cached/child")
        self.assertEqual(analyze_case_evidence(seal(events), evidence, PID)["verdict"], "SKIPPED")


class IdleTests(unittest.TestCase):
    def setup_case(self):
        case = "static-foreground"
        events = [session(), {"cat": "frame", "kind": "begin", "t": 8, "n": 1},
                  {"cat": "ui", "kind": "tail_repaint", "t": 8, "n": 1, "action": "none"},
                  *cached_request("dir::fixture/child", 3, 1, t=8),
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
        return seal(events), sampler, evidence, thresholds

    def test_sleeping_window_requires_external_identity_and_full_foreground_lifetime(self):
        events, sampler, evidence, thresholds = self.setup_case()
        self.assertEqual(analyze_idle_case(events, sampler, evidence, thresholds, PID)["verdict"], "PASS_WITH_WARNINGS")
        sampler["window_samples"][7]["foreground_pid"] = 99
        self.assertEqual(analyze_idle_case(events, sampler, evidence, thresholds, PID)["verdict"], "SKIPPED")

    def test_lifetime_gap_diagnostic_does_not_relax_coverage(self):
        for gap, expected in ((1100, "PASS_WITH_WARNINGS"), (1101, "SKIPPED"),
                              (1367, "SKIPPED")):
            with self.subTest(gap=gap):
                events, sampler, evidence, thresholds = self.setup_case()
                sampler["window_samples"][1]["unix_ms"] = sampler["window_samples"][0]["unix_ms"] + gap
                result = analyze_idle_case(events, sampler, evidence, thresholds, PID)
                self.assertEqual(result["verdict"], expected)
                self.assertEqual(result["metrics"]["window_sample_max_gap_ms"], gap)
                self.assertEqual(any("sample gap" in error for error in result["environment_errors"]), gap > 1100)
                self.assertFalse(any("complete measurement window" in error for error in result["environment_errors"]))

    def test_lifetime_endpoints_still_require_complete_measurement_window(self):
        for endpoint, delta in ((0, 100), (0, 101), (-1, -100), (-1, -101)):
            with self.subTest(endpoint=endpoint, delta=delta):
                events, sampler, evidence, thresholds = self.setup_case()
                sampler["window_samples"][endpoint]["unix_ms"] += delta
                result = analyze_idle_case(events, sampler, evidence, thresholds, PID)
                self.assertEqual(result["verdict"], "SKIPPED" if abs(delta) > 100 else "PASS_WITH_WARNINGS")
                self.assertEqual(any("complete measurement window" in error for error in result["environment_errors"]),
                                 abs(delta) > 100)

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
                    events = seal(events)
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

    def test_background_failure_preserves_clock_and_external_measurement_verdict(self):
        for cpu, expected in ((.01, "SKIPPED"), (.2, "FAIL")):
            with self.subTest(cpu=cpu):
                events, sampler, evidence, thresholds = self.setup_case()
                evidence["case_id"] = sampler["case_id"] = "static-background"
                next(e for e in events if e.get("kind") == "on_exit")["case_id"] = "static-background"
                # Reproduce the real run: all native samples still name the app,
                # but the independent sleep/clock evidence has been preserved.
                sampler.update(background_hwnd=456, background_process_id=99,
                               cpu_core_ratio=cpu, verdict="SKIPPED",
                               environment_errors=["Runner-owned background window condition was not maintained."])
                result = analyze_idle_case(events, sampler, evidence, thresholds, PID)
                self.assertEqual(result["verdict"], expected)
                self.assertTrue(result["evidence"]["clock"]["included"])
                self.assertTrue(any("background" in error for error in result["environment_errors"]))
                self.assertFalse(any("boundaries" in error for error in result["environment_errors"]))

    def test_static_requested_pixel_boundaries_and_real_worker_match(self):
        for pixels, expected in ((366, "PASS_WITH_WARNINGS"), (382, "PASS_WITH_WARNINGS"),
                                 (281, "SKIPPED"), (365, "SKIPPED"), (383, "SKIPPED")):
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

    def test_static_requires_canonical_begin_and_same_id_cache_origin(self):
        for kind, field, value in (("request_begin", "request_id", None), ("request_begin", "idx", True),
                                   ("cache_read", "request_id", 2), ("cache_read", "origin", "source_generated"),
                                   ("request_begin", "t", 8.1)):
            with self.subTest(kind=kind, field=field):
                events, sampler, evidence, thresholds = self.setup_case()
                next(e for e in events if e.get("kind") == kind)[field] = value
                self.assertEqual(analyze_idle_case(events, sampler, evidence, thresholds, PID)["verdict"], "SKIPPED")

    def test_passive_modifier_probe_uses_master_classifier_and_active_shape_still_fails(self):
        events, sampler, evidence, thresholds = self.setup_case()
        probe = {"cat": "input", "kind": "modifier_probe", "t": 11, "trigger": "heartbeat",
                 "keys": [], "wheel": False, "focused": False}
        events = seal([*events, probe])
        result = analyze_idle_case(events, sampler, evidence, thresholds, PID)
        self.assertEqual(result["verdict"], "PASS")
        self.assertEqual(result["metrics"]["idle"]["input_events"], 0)
        probe["wheel"] = 0
        result = analyze_idle_case(events, sampler, evidence, thresholds, PID)
        self.assertEqual(result["verdict"], "FAIL")
        self.assertEqual(result["metrics"]["idle"]["input_events"], 1)


class DeliveryTests(unittest.TestCase):
    def test_invalid_clock_retains_external_metrics_and_sampler_failure(self):
        for clock_gap in ("missing_anchor", "invalid_utc", "reversed_utc", "changed_wall_clock"):
            for failing_field, value in (("cpu_core_ratio", .5), ("app_log_growth_bytes", 16385),
                                         ("perf_log_growth_bytes", 262145), ("verdict", "FAIL"),
                                         (None, None)):
                with self.subTest(clock_gap=clock_gap, failing_field=failing_field):
                    events, sampler, evidence, thresholds = IdleTests().setup_case()
                    if clock_gap == "missing_anchor":
                        events[0].pop("wall_t")
                    elif clock_gap == "invalid_utc":
                        sampler["measure_start_unix_ms"] = None
                    elif clock_gap == "reversed_utc":
                        sampler["measure_end_unix_ms"] = sampler["measure_start_unix_ms"] - 1
                    else:
                        sampler["wall_seconds"] = 14
                    if failing_field:
                        sampler[failing_field] = value
                    if failing_field == "verdict":
                        sampler["failures"] = ["external sampler observed CPU failure"]
                    result = analyze_idle_case(events, sampler, evidence, thresholds, PID)
                    self.assertEqual((result["verdict"], result["exit_code"]),
                                     ("FAIL", 1) if failing_field else ("SKIPPED", 2))
                    self.assertTrue(result["environment_errors"])
                    self.assertNotIn("idle", result["metrics"])
                    for field in ("cpu_core_ratio", "app_log_growth_bytes", "perf_log_growth_bytes"):
                        self.assertEqual(result["metrics"][field], sampler[field])
                    if failing_field == "verdict":
                        self.assertIn("external sampler observed CPU failure", result["failures"])

    def test_missing_drop_counter_checkpoint_or_coverage_never_becomes_idle_pass(self):
        for field, value in (("coverage_t", 24), ("perf_dropped_total", 1),
                             ("normal_status", "unavailable"), ("written_watermark", 99),
                             ("session_id", "another-session"), ("diagnostic_dropped_total", None)):
            with self.subTest(field=field):
                events, sampler, evidence, thresholds = IdleTests().setup_case()
                events[-1][field] = value
                result = analyze_idle_case(events, sampler, evidence, thresholds, PID)
                self.assertEqual(result["verdict"], "SKIPPED")
                self.assertEqual(result["evidence"]["delivery"]["status"], "CannotJudge")
        events, sampler, evidence, thresholds = IdleTests().setup_case()
        self.assertEqual(analyze_idle_case(events[:-1], sampler, evidence, thresholds, PID)["verdict"], "SKIPPED")

    def test_incomplete_delivery_retains_independently_observed_cpu_and_input_failures(self):
        events, sampler, evidence, thresholds = IdleTests().setup_case()
        sampler["cpu_core_ratio"] = .10001
        result = analyze_idle_case(events[:-1], sampler, evidence, thresholds, PID)
        self.assertEqual((result["verdict"], result["exit_code"]), ("FAIL", 1))
        self.assertTrue(result["environment_errors"])
        events, sampler, evidence, thresholds = IdleTests().setup_case()
        events = [*events[:-1], {"cat": "input", "kind": "key", "t": 11}]
        result = analyze_idle_case(events, sampler, evidence, thresholds, PID)
        self.assertEqual((result["verdict"], result["exit_code"]), ("FAIL", 1))
        self.assertEqual(result["evidence"]["idle_delivery"]["status"], "CannotJudge")

    def test_perf_uses_shutdown_delivery_without_waiving_detected_performance_failure(self):
        events = perf_events()
        result = analyze_perf_smoke(events[:-1], expected_pid=PID, case_id="perf-cached", evidence=operations())
        self.assertEqual(result["verdict"], "SKIPPED")
        events = perf_events({0: .25})
        events[-1]["perf_dropped_total"] = 1
        result = analyze_perf_smoke(events, expected_pid=PID, case_id="perf-cached", evidence=operations())
        self.assertEqual(result["verdict"], "FAIL")
        self.assertTrue(result["environment_errors"])

    def test_strict_loader_retains_invalid_encoding_and_complete_json_without_newline(self):
        with test_directory() as path:
            log = path / "perf.jsonl"
            log.write_text(json.dumps(session()), encoding="utf-8")
            events, errors = load_events_strict(log)
            self.assertTrue(any("incomplete JSONL tail" in reason for reason in errors))
            self.assertTrue(events.evidence_errors)
            log.write_bytes(b'{"t":1,"cat":"qa","kind":"bad","key":"\xff"}\n')
            self.assertTrue(load_events_strict(log)[1])


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
