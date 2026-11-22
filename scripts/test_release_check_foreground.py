#!/usr/bin/env python3
"""Synthetic saved-evidence tests. No app, window access, or desktop input."""
from __future__ import annotations

import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path

from release_check_foreground import (
    FAILURE_FILE, MAX_GAP_MS, TIMELINE_FILE, classify_directory,
    classify_foreground_wait_failure, main,
)

PID, ROOT, CASE = 42, 100, "perf-cached"


def failure(**updates) -> dict:
    return {"schema_version": 1, "pid": PID, "root_hwnd": ROOT,
            "wait_kind": "exact-root-foreground", "wait_start_unix_ms": 2000,
            "failure_unix_ms": 4000, **updates}


def row(timestamp: int, **updates) -> dict:
    return {"schema_version": 1, "unix_ms": timestamp, "app_pid": PID,
            "root_hwnd": ROOT, "root_pid": PID, "root_alive": True,
            "root_visible": True, "root_minimized": False, "root_hung": False,
            "foreground_hwnd": 200, "foreground_pid": 99,
            "foreground_process_name": "Code.exe", "foreground_class": "Chrome_WidgetWin_1",
            "foreground_descendant": False, "ownership_resolved": True,
            "ghost_original_hwnd": None, "ghost_original_pid": None,
            "ghost_original_descendant": None, "ghost_original_process_name": None, **updates}


def timeline(**updates) -> list[dict]:
    return [row(timestamp, **updates) for timestamp in (1900, 2900, 3900, 4100)]


def ghost(**updates) -> list[dict]:
    return timeline(foreground_pid=500, foreground_process_name="DWM.EXE", foreground_class="Ghost",
                    ghost_original_hwnd=300, ghost_original_pid=999,
                    ghost_original_process_name="other.exe", ghost_original_descendant=False, **updates)


class ForegroundClassificationTests(unittest.TestCase):
    def classify(self, rows=None, record=None):
        return classify_foreground_wait_failure(
            failure() if record is None else record, timeline() if rows is None else rows, PID, CASE)

    def assert_failure(self, rows=None, record=None, reason=None):
        result = self.classify(rows, record)
        self.assertIs(result["environment"], False, result)
        if reason:
            self.assertIn(reason, result["reason"])
        return result

    def test_external_foreground_for_whole_wait_is_environment(self):
        for kind in ("exact-root-foreground", "ctrl-g-search-focus"):
            with self.subTest(kind=kind):
                result = self.classify(record=failure(wait_kind=kind))
                self.assertIs(result["environment"], True)
                self.assertEqual(result["details"]["selected_row_count"], 4)
                for detail in ("Code.exe", "pid=99", "Chrome_WidgetWin_1", "2000..4000", "rows=4"):
                    self.assertIn(detail, result["summary"])
                self.assertIn(f"kind={kind}", result["summary"])

    def test_application_same_pid_other_window_and_descendant_are_failures(self):
        for updates in ({"foreground_hwnd": ROOT, "foreground_pid": PID},
                        {"foreground_hwnd": 999, "foreground_pid": PID},
                        {"foreground_descendant": True}, {"foreground_hwnd": ROOT}):
            with self.subTest(updates=updates):
                self.assert_failure(timeline(**updates), reason="application or descendant")

    def test_mixed_external_and_application_interval_does_not_skip(self):
        for index in range(4):
            with self.subTest(index=index):
                rows = timeline()
                rows[index].update(foreground_pid=PID)
                self.assert_failure(rows)

    def test_ghost_other_process_is_environment_and_app_or_unknown_is_failure(self):
        resolved = self.classify(ghost())
        self.assertTrue(resolved["environment"])
        self.assertIn("original=other.exe (pid=999, hwnd=300)", resolved["summary"])
        for updates in ({"ghost_original_pid": PID}, {"ghost_original_descendant": True},
                        {"ghost_original_hwnd": ROOT}, {"ghost_original_pid": None},
                        {"ghost_original_hwnd": 0}, {"ghost_original_descendant": None},
                        {"ghost_original_process_name": ""}, {"foreground_process_name": "other.exe"}):
            with self.subTest(updates=updates):
                rows = ghost()
                rows[1].update(updates)
                self.assert_failure(rows)
        rows = ghost()
        for entry in rows:
            entry["foreground_process_name"] = "dwm"
        self.assertTrue(self.classify(rows)["environment"])

    def test_bad_root_health_at_any_boundary_is_failure(self):
        for field, value in (("root_hung", True), ("root_minimized", True),
                             ("root_alive", False), ("root_visible", False), ("root_hung", None)):
            for index in (0, 1, 3):
                with self.subTest(field=field, index=index):
                    rows = timeline()
                    rows[index][field] = value
                    self.assert_failure(rows, reason="application root")

    def test_missing_unresolved_or_zero_foreground_is_failure(self):
        for field, value in (("foreground_hwnd", 0), ("foreground_pid", 0),
                             ("foreground_descendant", None), ("ownership_resolved", False),
                             ("foreground_process_name", ""), ("foreground_class", None)):
            with self.subTest(field=field):
                self.assert_failure(timeline(**{field: value}), reason="unresolved")

    def test_root_and_application_exact_identity_is_required(self):
        for field, value in (("app_pid", 5), ("root_pid", 99), ("root_hwnd", ROOT + 1),
                             ("app_pid", True), ("root_pid", None)):
            with self.subTest(field=field):
                self.assert_failure(timeline(**{field: value}), reason="ownership mismatch")

    def test_whole_interval_and_maximum_gap_required(self):
        self.assert_failure([], reason="missing or empty")
        self.assert_failure(timeline()[1:], reason="whole wait")
        self.assert_failure(timeline()[:-1], reason="whole wait")
        self.assert_failure([row(1900), row(3900), row(4100)], reason="gap exceeds")
        self.assertTrue(self.classify([row(2000), row(2000 + MAX_GAP_MS), row(4000)])["environment"])
        self.assert_failure([row(2000), row(2001 + MAX_GAP_MS), row(4000)], reason="gap exceeds")

    def test_samples_bracketing_the_wait_are_checked_conservatively(self):
        rows = [row(1000, root_hung=True), *timeline(), row(5000, foreground_pid=PID)]
        result = self.classify(rows)
        self.assertTrue(result["environment"])
        self.assertEqual({entry["foreground_pid"] for entry in result["details"]["foreground"]}, {99})
        rows[1]["root_hung"] = True
        self.assert_failure(rows)

    def test_schema_and_strict_timestamp_order(self):
        for rows in ([row(2000), row(2000), row(4000)],
                     [row(3000), row(2000), row(4000)],
                     [row(0), *timeline()], [None, *timeline()],
                     [row(1000, schema_version=True), *timeline()]):
            with self.subTest(rows=rows):
                self.assert_failure(rows)

    def test_invalid_failure_record_never_classifies_environment(self):
        for record in ({}, [], None, failure(pid=99), failure(pid=True), failure(root_hwnd=0),
                       failure(wait_kind="other"), failure(wait_kind=[]), failure(schema_version=True),
                       failure(wait_start_unix_ms=0), failure(failure_unix_ms=1999),
                       failure(wait_start_unix_ms="2000")):
            with self.subTest(record=record):
                result = classify_foreground_wait_failure(record, timeline(), PID, CASE)
                self.assertFalse(result["environment"])
                self.assertIn("Code.exe", result["summary"])
        self.assertFalse(classify_foreground_wait_failure(failure(), timeline(), True, CASE)["environment"])

    def test_missing_fields_do_not_coerce_to_safe_defaults(self):
        for field in row(1900):
            if field.startswith("ghost_"):
                continue
            with self.subTest(field=field):
                rows = timeline()
                del rows[1][field]
                self.assert_failure(rows)


class ForegroundCliTests(unittest.TestCase):
    def write_evidence(self, directory: Path):
        (directory / FAILURE_FILE).write_text(json.dumps(failure()), encoding="utf-8")
        (directory / TIMELINE_FILE).write_text(
            "".join(json.dumps(entry) + "\n" for entry in timeline()), encoding="utf-8")

    def test_cli_prints_one_ascii_json_decision_exit_zero_without_writes(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.write_evidence(directory)
            before = {p.name: p.read_bytes() for p in directory.iterdir()}
            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout):
                code = main(["--directory", str(directory), "--expected-pid", str(PID), "--case-id", CASE])
            self.assertEqual(code, 0)
            self.assertTrue(json.loads(stdout.getvalue())["environment"])
            stdout.getvalue().encode("ascii")
            self.assertEqual(before, {p.name: p.read_bytes() for p in directory.iterdir()})

    def test_missing_failure_is_fail_and_cli_still_exit_zero(self):
        with tempfile.TemporaryDirectory() as temporary:
            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout):
                code = main(["--directory", temporary, "--expected-pid", str(PID), "--case-id", CASE])
            result = json.loads(stdout.getvalue())
            self.assertEqual(code, 0)
            self.assertFalse(result["environment"])
            self.assertIn(FAILURE_FILE, result["reason"])

    def test_malformed_or_truncated_timeline_and_record_are_fail(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            for malformed in ("not-json\n", "[]\n", "\n", '{"unix_ms": NaN}\n', json.dumps(row(2500))):
                with self.subTest(malformed=malformed):
                    self.write_evidence(directory)
                    with (directory / TIMELINE_FILE).open("a", encoding="utf-8") as stream:
                        stream.write(malformed)
                    result = classify_directory(directory, PID, CASE)
                    self.assertFalse(result["environment"])
                    self.assertIn("read_errors", result["details"])
            for malformed in ("not-json", "[]", '{"pid": NaN}'):
                with self.subTest(malformed=malformed):
                    self.write_evidence(directory)
                    (directory / FAILURE_FILE).write_text(malformed, encoding="utf-8")
                    self.assertFalse(classify_directory(directory, PID, CASE)["environment"])


if __name__ == "__main__":
    unittest.main()
