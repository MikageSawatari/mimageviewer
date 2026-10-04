#!/usr/bin/env python3
"""analyze_perf.py の標準ライブラリだけで動く回帰テスト。"""

from __future__ import annotations

import contextlib
import io
import json
import shutil
import sys
import unittest
import uuid
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

from analyze_perf import (
    analyze_collection,
    analyze_thumb_adjustment_frames,
    analyze_page_turn,
    analyze_remote_page,
    analyze_test_script_input,
    analyze_idle_health,
    cmd_colorize,
    cmd_collection,
    cmd_idle_health,
    cmd_hitches,
    cmd_metadata_search,
    cmd_memory,
    cmd_page_turn,
    cmd_pre_grid,
    cmd_remote_page,
    cmd_thumbs,
    load_events,
    main,
    percentile,
)


@contextlib.contextmanager
def writable_test_directory():
    """Avoid Python 3.13's owner-only Windows temp ACL in restricted runners."""
    base = Path(__file__).resolve().parent.parent / "target" / "test-analyze-perf"
    base.mkdir(parents=True, exist_ok=True)
    path = base / f"run-{uuid.uuid4().hex}"
    path.mkdir()
    try:
        yield path
    finally:
        shutil.rmtree(path, ignore_errors=True)


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


def collection_lease(
    t: float,
    request_id: int,
    outcome: str,
    *,
    owner: str = "grid",
    phase: str = "snapshot",
    active_ms: float = 0.0,
    wall_ms: float = 0.0,
    context: int | None = None,
    surface_generation: int | None = None,
) -> dict:
    event = {
        "t": t,
        "cat": "collection",
        "kind": "read_lease",
        "request_id": request_id,
        "owner": owner,
        "phase": phase,
        "active_ms": active_ms,
        "wall_ms": wall_ms,
        "outcome": outcome,
    }
    if context is not None:
        event["context"] = context
    if surface_generation is not None:
        event["surface_generation"] = surface_generation
    return event


def thumb(t: float, kind: str, key: str) -> dict:
    return {
        "t": t,
        "cat": "thumb",
        "kind": kind,
        "key": key,
        "idx": 7,
        "items_gen": 2,
    }


def load_folder_begin(t: float, path: str = "C:/books") -> dict:
    return {
        "t": t,
        "cat": "nav",
        "kind": "load_folder_begin",
        "path": path,
    }


def page_turn(
    t: float,
    idx: int,
    mode: str,
    generation: int = 2,
    source: str = "thumbnail",
) -> dict:
    return {
        "t": t,
        "cat": "fs",
        "kind": "page_turn_ready",
        "idx": idx,
        "items_generation": generation,
        "mode": mode,
        "source": source,
    }


def hold_begin(t: float, hold_id: int, key: str = "Right") -> dict:
    return {
        "t": t,
        "cat": "test_script",
        "kind": "hold_begin",
        "hold_id": hold_id,
        "key": key,
        "target_viewport": "ROOT",
        "repeat_delay_ms": 250.0,
        "repeat_hz": 30.0,
    }


def hold_end(t: float, hold_id: int) -> dict:
    return {
        "t": t,
        "cat": "test_script",
        "kind": "hold_end",
        "hold_id": hold_id,
        "down_count": 1,
        "repeat_count": 5,
        "up_count": 1,
    }


def frame_input(
    t: float,
    hold_id: int,
    *,
    held: bool,
    edge_count: int,
    materialized_in_frame: int,
    frame_nr: int,
) -> dict:
    return {
        "t": t,
        "cat": "test_script",
        "kind": "frame_input",
        "hold_id": hold_id,
        "held": held,
        "edge_count": edge_count,
        "materialized_in_frame": materialized_in_frame,
        "frame_nr": frame_nr,
    }


def level_read(
    t: float,
    hold_id: int,
    *,
    held: bool,
    frame_nr: int,
) -> dict:
    return {
        "t": t,
        "cat": "test_script",
        "kind": "level_read",
        "hold_id": hold_id,
        "held": held,
        "frame_nr": frame_nr,
        "reader": "Keymap::key_held_chord",
    }


def add_level_reads(events: list[dict]) -> list[dict]:
    result = list(events)
    result.extend(
        level_read(
            float(event.get("t", 0.0)),
            int(event["hold_id"]),
            held=True,
            frame_nr=int(event["frame_nr"]),
        )
        for event in events
        if event.get("cat") == "test_script"
        and event.get("kind") == "frame_input"
        and event.get("held") is True
    )
    return result


def valid_test_script_input(hold_id: int = 900) -> list[dict]:
    # No hold_begin/end here: invariant-only tests keep exercising the legacy
    # time-gap splitter while satisfying the independent harness evidence gate.
    return add_level_reads([
        frame_input(
            0.10,
            hold_id,
            held=True,
            edge_count=1,
            materialized_in_frame=0,
            frame_nr=1,
        ),
        frame_input(
            0.12,
            hold_id,
            held=True,
            edge_count=0,
            materialized_in_frame=0,
            frame_nr=2,
        ),
        frame_input(
            0.14,
            hold_id,
            held=True,
            edge_count=3,
            materialized_in_frame=3,
            frame_nr=3,
        ),
    ])


def page_turn_gate_decision(
    t: float,
    idx: int,
    defer_ui_uploads: bool,
    generation: int = 2,
    reason: str = "pass_through",
    passthrough_rendition_ready: bool = True,
    ui_work_admission: str | None = None,
) -> dict:
    event = {
        "t": t,
        "cat": "fs",
        "kind": "page_turn_decision",
        "idx": idx,
        "items_generation": generation,
        "reason": reason,
        "passthrough_rendition_ready": passthrough_rendition_ready,
        "defer_ui_uploads": defer_ui_uploads,
    }
    if ui_work_admission is not None:
        event["ui_work_admission"] = ui_work_admission
    return event


def page_turn_decision(
    t: float,
    frame_number: int,
    reason: str,
    pending: int,
    matching: int,
    chords: str = "",
) -> dict:
    return {
        "t": t,
        "cat": "fs",
        "kind": "page_turn_decision",
        "n": frame_number,
        "frame_nr": frame_number,
        "idx": 7,
        "reason": reason,
        "ordinary_blocker": "none",
        "win32_pending_page_turn_edge_count": pending,
        "win32_pending_page_turn_repeat_count": pending,
        "win32_matching_page_turn_edge_count": matching,
        "win32_matching_page_turn_chords": chords,
    }


def page_turn_egui_probe(t: float, frame_number: int, count: int) -> dict:
    return {
        "t": t,
        "cat": "fs",
        "kind": "page_turn_egui_input",
        "n": frame_number,
        "frame_nr": frame_number,
        "source": "fullscreen",
        "egui_page_turn_event_count": count,
        "egui_page_turn_repeat_count": count,
        "egui_page_turn_chords": f"Left={count}/{count}r",
    }


def page_turn_winit_probe(t: float, frame_number: int, count: int) -> dict:
    return {
        "t": t,
        "cat": "fs",
        "kind": "page_turn_winit_input",
        "frame_nr": frame_number,
        "viewport": "FFFF",
        "winit_page_turn_event_count": count,
        "winit_page_turn_repeat_count": count,
        "winit_page_turn_chords": f"Left={count}/{count}r",
    }


def remote_page_stage(
    stage: str,
    ms: float,
    others: int,
    *,
    pixels: int = 1_000_000,
    wait_ms: float = 0.0,
    outcome: str = "ok",
) -> dict:
    return {
        "t": 1.0,
        "cat": "remote_page",
        "kind": "stage",
        "stage": stage,
        "ms": ms,
        "wait_ms": wait_ms,
        "active_others": others,
        "active_total": others + 1,
        "pixels": pixels,
        "bytes": pixels * 4,
        "outcome": outcome,
    }


class CollectionReportTests(unittest.TestCase):
    def test_request_identity_is_scoped_to_session_and_late_viewer_binding(self) -> None:
        events = [
            session(101),
            collection_lease(0.01, 7, "begin", phase="admission"),
            collection_lease(
                0.02,
                7,
                "phase",
                context=11,
                surface_generation=3,
            ),
            {
                "t": 0.03,
                "cat": "collection",
                "kind": "navigation_begin",
                "request_id": 7,
                "surface_generation": 3,
            },
            collection_lease(
                0.04,
                7,
                "ready",
                active_ms=30.0,
                wall_ms=30.0,
                context=11,
                surface_generation=3,
            ),
            session(202),
            collection_lease(0.01, 7, "begin", owner="manager", phase="catalog"),
            collection_lease(
                0.05,
                7,
                "error",
                owner="manager",
                phase="catalog",
                active_ms=40.0,
                wall_ms=40.0,
            ),
        ]

        report = analyze_collection(events)

        self.assertEqual(report["sessions"], 2)
        self.assertEqual(report["lease"]["completed"], 2)
        self.assertEqual(report["lease"]["terminal_outcomes"], {"ready": 1, "error": 1})
        self.assertEqual(report["lease"]["active"]["p50_ms"], 35.0)
        self.assertEqual(report["legacy"]["matched_by_request_id"], 1)
        self.assertEqual(report["legacy"]["unmatched"], 0)
        self.assertEqual(
            [row["session"] for row in report["lease"]["completed_requests"]],
            [0, 1],
            "the same request id in another process session is a different request",
        )

    def test_partial_stale_conflicting_and_unknown_records_never_become_success_or_hang(self) -> None:
        missing_owner = collection_lease(0.11, 7, "phase")
        missing_owner.pop("owner")
        events = [
            collection_lease(9.0, 1, "begin"),
            collection_lease(9.1, 1, "ready", active_ms=100.0, wall_ms=100.0),
            session(303),
            collection_lease(0.01, 2, "ready", active_ms=10.0, wall_ms=10.0),
            collection_lease(0.02, 3, "begin"),
            collection_lease(0.03, 4, "begin"),
            collection_lease(0.04, 4, "phase", context=1, surface_generation=8),
            collection_lease(0.05, 4, "phase", context=2, surface_generation=8),
            collection_lease(
                0.06,
                4,
                "ready",
                context=2,
                surface_generation=8,
            ),
            collection_lease(0.07, 5, "begin"),
            collection_lease(0.08, 5, "future_terminal"),
            collection_lease(0.09, 6, "begin", owner="grid"),
            collection_lease(0.10, 6, "phase", owner="manager"),
            collection_lease(0.11, 6, "ready", owner="manager"),
            collection_lease(0.10, 7, "begin", owner="grid"),
            missing_owner,
            collection_lease(0.12, 7, "ready", owner="grid"),
        ]

        report = analyze_collection(events)["lease"]

        self.assertEqual(report["completed"], 0)
        self.assertEqual(report["terminal_outcomes"], {})
        self.assertEqual(report["partial_session"], 1)
        self.assertEqual(report["unmatched"], 1)
        self.assertEqual(report["incomplete"], 1)
        self.assertEqual(report["scope_conflict"], 3)
        self.assertEqual(report["unknown_terminal"], 1)

        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            cmd_collection(events)
        rendered = output.getvalue()
        self.assertIn("diagnostic only", rendered)
        self.assertIn("does not infer success or a hang", rendered)

    def test_legacy_stages_are_summarized_without_uuid_or_timestamp_joining(self) -> None:
        events = [
            session(404),
            collection_lease(
                0.01,
                9,
                "begin",
                context=4,
                surface_generation=12,
            ),
            {
                "t": 0.02,
                "cat": "collection",
                "kind": "prepare",
                "stage": "identity",
                "request_id": 9,
                "outcome": "ok",
                "ms": 10.0,
            },
            {
                "t": 0.03,
                "cat": "collection",
                "kind": "prepare",
                "stage": "identity",
                "collection_id": "same-uuid-is-not-a-join-key",
                "outcome": "error",
                "ms": 30.0,
            },
            {
                "t": 0.04,
                "cat": "collection",
                "kind": "navigation_begin",
                "request_id": 9,
                "surface_generation": 99,
            },
            collection_lease(
                0.05,
                9,
                "ready",
                active_ms=40.0,
                wall_ms=40.0,
                context=4,
                surface_generation=12,
            ),
        ]

        report = analyze_collection(events)
        identity = report["legacy"]["stages"]["prepare/identity"]

        self.assertEqual(identity["count"], 2)
        self.assertEqual(identity["outcomes"], {"ok": 1, "error": 1})
        self.assertEqual(identity["duration"]["p50_ms"], 20.0)
        self.assertEqual(identity["duration"]["p95_ms"], 29.0)
        self.assertEqual(identity["duration"]["max_ms"], 30.0)
        self.assertEqual(report["legacy"]["matched_by_request_id"], 1)
        self.assertEqual(report["legacy"]["unmatched"], 2)
        self.assertEqual(report["legacy"]["scope_conflict"], 1)


class MetadataSearchReportTests(unittest.TestCase):
    def test_reports_pass_timings_bytes_and_unmeasured_video_reads(self):
        events = [
            {
                "t": 1.0,
                "cat": "search",
                "kind": "metadata_filter_done",
                "pass1_ms": 2.0,
                "pass2_ms": 8.0,
                "total_ms": 10.0,
                "items_scanned": 20,
                "items_total": 20,
                "pass2_file_reads": 12,
                "bytes_read": 1024 * 1024,
                "matches": 3,
                "cancelled": False,
                "unmeasured_video_reads": 1,
            }
        ]
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            cmd_metadata_search(events)
        report = out.getvalue()
        self.assertIn("10.0", report)
        self.assertIn("1.000 MiB", report)
        self.assertIn("動画 read 1 件", report)


class RemotePageReportTests(unittest.TestCase):
    def test_percentile_uses_interpolated_median_and_p90(self) -> None:
        self.assertEqual(percentile([100.0, 180.0], 0.5), 140.0)
        self.assertEqual(percentile([100.0, 180.0], 0.9), 172.0)

    def test_groups_successful_stages_by_concurrency_and_keeps_outcomes(self) -> None:
        events = [
            remote_page_stage("source", 100.0, 0, wait_ms=0.1),
            remote_page_stage("source", 180.0, 1, wait_ms=12.0),
            remote_page_stage(
                "source", 0.01, 0, outcome="composite_cache_hit"
            ),
            remote_page_stage("jpeg", 40.0, 0, pixels=2_000_000),
        ]

        report = analyze_remote_page(events)

        self.assertEqual(len(report["by_stage"]["source"]), 3)
        self.assertEqual(len(report["by_stage_and_others"][("source", 1)]), 1)
        self.assertEqual(report["outcomes"][("source", "ok")], 2)
        self.assertEqual(
            report["outcomes"][("source", "composite_cache_hit")], 1
        )

    def test_prints_pixel_cost_concurrency_and_lock_wait(self) -> None:
        events = [
            remote_page_stage("source", 100.0, 0, wait_ms=0.1),
            remote_page_stage("source", 180.0, 1, wait_ms=12.0),
            remote_page_stage("jpeg", 40.0, 0, pixels=2_000_000),
        ]
        stdout = io.StringIO()

        with contextlib.redirect_stdout(stdout):
            cmd_remote_page(events)

        output = stdout.getvalue()
        self.assertIn("Remote page stages", output)
        self.assertIn("ms/MP", output)
        self.assertIn("Stage duration by active_others", output)
        self.assertIn("source          1", output)
        self.assertIn("Instrumented lock wait", output)


class PageTurnTests(unittest.TestCase):
    def test_groups_pass_pages_with_the_materialized_stop_page(self) -> None:
        report = analyze_page_turn([
            page_turn(1.000, 10, "pass_through"),
            page_turn(1.034, 11, "pass_through"),
            page_turn(1.069, 12, "pass_through"),
            page_turn(1.105, 13, "materialized"),
            page_turn(2.000, 20, "materialized"),
        ])

        self.assertEqual(
            report["counts"],
            {"pass_through": 3, "materialized": 2},
        )
        self.assertEqual(len(report["holds"]), 1)
        hold = report["holds"][0]
        self.assertTrue(hold["complete"])
        self.assertEqual(hold["indices"], [10, 11, 12, 13])
        self.assertEqual(hold["pass_through"], 3)
        self.assertEqual(hold["materialized"], 1)
        self.assertAlmostEqual(hold["intervals_ms"][0], 34.0)

    def test_generation_change_closes_an_incomplete_hold(self) -> None:
        report = analyze_page_turn([
            page_turn(1.000, 4, "pass_through", generation=2),
            page_turn(1.020, 0, "pass_through", generation=3),
            page_turn(1.050, 1, "materialized", generation=3),
        ])

        self.assertEqual(len(report["holds"]), 2)
        self.assertFalse(report["holds"][0]["complete"])
        self.assertTrue(report["holds"][1]["complete"])

    def test_reports_false_reason_and_three_input_stage_cardinalities(self) -> None:
        events = [
            page_turn_decision(1.000, 40, "pending_zero", 0, 0),
            page_turn_winit_probe(1.001, 40, 1),
            page_turn_egui_probe(1.001, 40, 1),
            page_turn_decision(1.034, 41, "pass_through", 2, 2, "Left=2/2r"),
            page_turn_winit_probe(1.035, 41, 1),
            page_turn_egui_probe(1.035, 41, 1),
        ]
        report = analyze_page_turn(events)

        self.assertEqual(
            report["decision_reasons"],
            {"pending_zero": 1, "pass_through": 1},
        )
        self.assertEqual(len(report["diagnostic_decisions"]), 2)
        self.assertEqual(
            report["diagnostic_decisions"][0]["winit_probes"][0]
            ["winit_page_turn_event_count"],
            1,
        )
        self.assertEqual(
            report["diagnostic_decisions"][0]["egui_probes"][0]
            ["egui_page_turn_event_count"],
            1,
        )

        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            cmd_page_turn(events)
        rendered = output.getvalue()
        self.assertIn("pending_zero=1", rendered)
        self.assertIn("Win32 pending/repeat/matching", rendered)
        self.assertIn("winit press/repeat", rendered)
        self.assertIn("0/0/0 | 1/1 | 1/1", rendered)


class PageTurnInvariantCliTests(unittest.TestCase):
    def run_page_turn(
        self,
        events: list[dict],
        *,
        check: bool = True,
        input_evidence: bool = True,
    ) -> SimpleNamespace:
        if check and input_evidence:
            events = list(events) + valid_test_script_input()
        synthetic_jsonl = (
            "\n".join(json.dumps(event) for event in events) + "\n"
        )
        argv = ["analyze_perf.py", "page-turn.jsonl", "page-turn"]
        if check:
            argv.append("--check")
        stdout = io.StringIO()
        stderr = io.StringIO()
        exit_code = 0
        with (
            mock.patch.object(sys, "argv", argv),
            mock.patch.object(Path, "is_file", return_value=True),
            mock.patch.object(
                Path,
                "open",
                return_value=io.StringIO(synthetic_jsonl),
            ),
            contextlib.redirect_stdout(stdout),
            contextlib.redirect_stderr(stderr),
        ):
            try:
                main()
            except SystemExit as error:
                exit_code = int(error.code or 0)
        return SimpleNamespace(
            returncode=exit_code,
            stdout=stdout.getvalue(),
            stderr=stderr.getvalue(),
        )

    def test_i1_through_i5_each_have_failing_and_passing_traces(self) -> None:
        common_good = [
            page_turn(1.0, 1, "materialized"),
            page_turn(1.1, 2, "materialized"),
        ]
        cases = {
            "I1": (
                [
                    page_turn(
                        1.0,
                        1,
                        "materialized",
                        source="final_composite",
                    ),
                    page_turn(1.1, 1, "materialized", source="thumbnail"),
                ],
                [
                    page_turn(
                        1.0,
                        1,
                        "materialized",
                        source="thumbnail",
                    ),
                    page_turn(
                        1.1,
                        1,
                        "materialized",
                        source="final_composite",
                    ),
                ],
            ),
            "I2": (
                [
                    page_turn(
                        1.0,
                        1,
                        "materialized",
                        source="final_composite",
                    ),
                    page_turn(1.1, 2, "materialized", source="thumbnail"),
                ],
                common_good,
            ),
            "I3": (
                [
                    page_turn(1.0, 1, "materialized"),
                    page_turn(1.1, 3, "materialized"),
                ],
                [
                    page_turn(1.0, 1, "materialized"),
                    page_turn(1.1, 2, "materialized"),
                    page_turn(1.2, 3, "materialized"),
                ],
            ),
            "I4": (
                [
                    page_turn(1.0, 1, "materialized"),
                    page_turn(1.1, 2, "pass_through"),
                ],
                common_good,
            ),
            "I5": (
                common_good
                + [
                    page_turn_gate_decision(
                        1.05,
                        2,
                        defer_ui_uploads=True,
                        ui_work_admission="all",
                    )
                ],
                common_good
                + [
                    page_turn_gate_decision(
                        1.05,
                        2,
                        defer_ui_uploads=True,
                        ui_work_admission="deferred",
                    )
                ],
            ),
        }

        for invariant, (failing_events, passing_events) in cases.items():
            with self.subTest(invariant=invariant, status="fail"):
                result = self.run_page_turn(failing_events)
                self.assertEqual(result.returncode, 1, result.stdout)
                self.assertIn(f"{invariant} violation:", result.stdout)
                self.assertIn("source sequence:", result.stdout)
            with self.subTest(invariant=invariant, status="pass"):
                result = self.run_page_turn(passing_events)
                self.assertEqual(result.returncode, 0, result.stdout)
                self.assertIn("checked bursts=1 violations=0", result.stdout)

    def test_bursts_split_after_300ms_and_on_generation_change(self) -> None:
        split_cases = {
            "gap": [
                page_turn(1.0, 1, "materialized"),
                page_turn(1.301, 3, "materialized"),
            ],
            "generation": [
                page_turn(1.0, 1, "materialized", generation=2),
                page_turn(1.1, 3, "materialized", generation=3),
            ],
        }
        for split, events in split_cases.items():
            with self.subTest(split=split):
                result = self.run_page_turn(events)
                self.assertEqual(result.returncode, 0, result.stdout)
                self.assertIn("checked bursts=2 violations=0", result.stdout)

        inclusive = self.run_page_turn([
            page_turn(1.0, 1, "materialized"),
            page_turn(1.3, 3, "materialized"),
        ])
        self.assertEqual(inclusive.returncode, 1, inclusive.stdout)
        self.assertIn("I3 violation:", inclusive.stdout)
        self.assertIn("checked bursts=1", inclusive.stdout)

    def test_hold_id_keeps_463ms_ready_events_in_one_burst(self) -> None:
        hold_id = 17
        events = [
            hold_begin(0.5, hold_id),
            page_turn(1.000, 1, "materialized"),
            page_turn(1.463, 2, "materialized"),
            hold_end(2.0, hold_id),
        ] + valid_test_script_input(hold_id)

        result = self.run_page_turn(events, input_evidence=False)

        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("page-turn burst split: hold_id", result.stdout)
        self.assertIn("checked bursts=1 violations=0", result.stdout)

    def test_hold_id_boundary_splits_nearby_ready_events(self) -> None:
        events = [
            hold_begin(0.5, 17),
            page_turn(1.000, 1, "materialized"),
            hold_end(1.010, 17),
            hold_begin(1.020, 18),
            page_turn(1.100, 3, "materialized"),
            hold_end(1.200, 18),
        ] + valid_test_script_input(17)

        result = self.run_page_turn(events, input_evidence=False)

        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("page-turn burst split: hold_id", result.stdout)
        self.assertIn("checked bursts=2 violations=0", result.stdout)

    def test_hold_mode_ignores_ready_events_outside_down_up_ranges(self) -> None:
        events = [
            page_turn(0.400, 10, "materialized"),
            hold_begin(0.500, 17),
            page_turn(0.600, 1, "materialized"),
            hold_end(0.700, 17),
            page_turn(0.800, 20, "materialized"),
        ] + valid_test_script_input(17)

        result = self.run_page_turn(events, input_evidence=False)

        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("page-turn burst split: hold_id", result.stdout)
        self.assertIn("checked bursts=1 violations=0", result.stdout)

    def test_without_hold_events_keeps_legacy_300ms_burst_split(self) -> None:
        result = self.run_page_turn([
            page_turn(1.000, 1, "materialized"),
            page_turn(1.463, 3, "materialized"),
        ])

        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("page-turn burst split: time_gap (legacy 300ms;", result.stdout)
        self.assertIn("checked bursts=2 violations=0", result.stdout)

    def test_i2_exempts_only_passthrough_rendition_unavailable_mix(self) -> None:
        unavailable_mix = [
            page_turn_gate_decision(0.99, 1, defer_ui_uploads=True),
            page_turn(1.0, 1, "pass_through", source="thumbnail"),
            page_turn_gate_decision(
                1.09,
                2,
                defer_ui_uploads=False,
                reason="passthrough_rendition_unavailable",
                passthrough_rendition_ready=False,
            ),
            page_turn(1.1, 2, "materialized", source="final_composite"),
        ]
        result = self.run_page_turn(unavailable_mix)
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertNotIn("I2 violation:", result.stdout)

        unexplained_mix = list(unavailable_mix)
        unexplained_mix[2] = page_turn_gate_decision(
            1.09,
            2,
            defer_ui_uploads=False,
            reason="pending_zero",
            passthrough_rendition_ready=False,
        )
        result = self.run_page_turn(unexplained_mix)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("I2 violation:", result.stdout)

    def test_i3_is_skipped_for_a_direction_unknown_burst(self) -> None:
        result = self.run_page_turn([
            page_turn(1.0, 1, "materialized"),
            page_turn(1.1, 4, "materialized"),
            page_turn(1.2, 3, "materialized"),
        ])

        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertNotIn("I3 violation:", result.stdout)

    def test_i4_endpoint_requires_materialized_settle(self) -> None:
        endpoint_cases = {
            "last": [
                page_turn(1.0, 1, "materialized"),
                page_turn(1.1, 2, "pass_through"),
                page_turn(1.2, 2, "pass_through"),
            ],
            "first": [
                page_turn(1.0, 1, "materialized"),
                page_turn(1.1, 0, "pass_through"),
                page_turn(1.2, 0, "pass_through"),
            ],
        }
        for endpoint, events in endpoint_cases.items():
            with self.subTest(endpoint=endpoint):
                result = self.run_page_turn(events)
                self.assertEqual(result.returncode, 1, result.stdout)
                self.assertIn("I4 violation:", result.stdout)

        settled_endpoint = self.run_page_turn([
            page_turn(1.0, 1, "materialized"),
            page_turn(1.1, 2, "pass_through"),
            page_turn(1.2, 2, "materialized"),
        ])
        self.assertEqual(settled_endpoint.returncode, 0, settled_endpoint.stdout)
        self.assertNotIn("I4 violation:", settled_endpoint.stdout)

    def test_i5_only_applies_to_rendition_ready_pending_frames(self) -> None:
        events = [
            page_turn(1.0, 1, "materialized"),
            page_turn(1.1, 2, "materialized"),
            page_turn_gate_decision(
                1.02,
                1,
                defer_ui_uploads=False,
                reason="pending_zero",
            ),
            page_turn_gate_decision(
                1.04,
                1,
                defer_ui_uploads=False,
                passthrough_rendition_ready=False,
            ),
            page_turn_gate_decision(1.06, 2, defer_ui_uploads=True),
        ]

        result = self.run_page_turn(events)

        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertNotIn("I5 violation:", result.stdout)

    def test_check_without_page_turn_events_is_not_exercised(self) -> None:
        result = self.run_page_turn([
            {"t": 1.0, "cat": "fs", "kind": "paint", "idx": 3},
        ])

        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("checked bursts=0 violations=0", result.stdout)
        self.assertIn("page-turn invariants: status=not-exercised", result.stdout)

    def test_page_turn_check_rejects_missing_harness_evidence(self) -> None:
        result = self.run_page_turn(
            [page_turn(1.0, 1, "materialized")],
            input_evidence=False,
        )

        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("test-script input: status=not-established", result.stdout)

    def test_without_check_keeps_the_existing_success_output(self) -> None:
        result = self.run_page_turn([
            page_turn(1.0, 1, "materialized"),
            page_turn(1.1, 3, "materialized"),
        ], check=False)

        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertEqual(
            result.stdout,
            "page-turn ready: pass_through=0 materialized=2\n"
            "(pass_through を含むキーリピート区間なし)\n",
        )


class TestScriptInputGateTests(unittest.TestCase):
    def run_input_check(self, events: list[dict]) -> SimpleNamespace:
        synthetic_jsonl = "\n".join(json.dumps(event) for event in events) + "\n"
        stdout = io.StringIO()
        exit_code = 0
        with (
            mock.patch.object(
                sys,
                "argv",
                [
                    "analyze_perf.py",
                    "test-script.jsonl",
                    "test-script-input",
                    "--check",
                ],
            ),
            mock.patch.object(Path, "is_file", return_value=True),
            mock.patch.object(
                Path,
                "open",
                return_value=io.StringIO(synthetic_jsonl),
            ),
            contextlib.redirect_stdout(stdout),
        ):
            try:
                main()
            except SystemExit as error:
                exit_code = int(error.code or 0)
        return SimpleNamespace(returncode=exit_code, stdout=stdout.getvalue())

    def test_missing_vibration_is_not_established(self) -> None:
        missing_vibration = [
            frame_input(
                1.0,
                1,
                held=True,
                edge_count=2,
                materialized_in_frame=2,
                frame_nr=1,
            ),
        ]

        report = analyze_test_script_input(missing_vibration)
        self.assertEqual(report["status"], "not-established")
        result = self.run_input_check(missing_vibration)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("vibration=no", result.stdout)

    def test_missing_accumulation_alone_still_establishes_the_harness(self) -> None:
        # Accumulation only happens when a frame outlasts the repeat interval,
        # which depends on how heavy the book is rather than on the harness. A
        # 1.6MP folder renders at ~6ms and can never produce one, so requiring
        # it here failed correct runs.
        events = [
            frame_input(
                1.0,
                1,
                held=True,
                edge_count=1,
                materialized_in_frame=0,
                frame_nr=1,
            ),
            frame_input(
                1.1,
                1,
                held=True,
                edge_count=0,
                materialized_in_frame=0,
                frame_nr=2,
            ),
            page_turn(1.05, 1, "materialized"),
        ]

        report = analyze_test_script_input(add_level_reads(events))
        self.assertEqual(report["status"], "pass")
        self.assertFalse(report["accumulated_frames"])

    def test_fast_run_without_page_turns_does_not_need_accumulation(self) -> None:
        # A frame only absorbs several repeats when it outlasts the repeat
        # interval. A real 2.5s hold on the grid ran at ~166fps and accumulated
        # nothing, so requiring it here would call a working harness broken.
        events = add_level_reads([
            frame_input(
                1.0,
                1,
                held=True,
                edge_count=1,
                materialized_in_frame=0,
                frame_nr=1,
            ),
            frame_input(
                1.1,
                1,
                held=True,
                edge_count=0,
                materialized_in_frame=0,
                frame_nr=2,
            ),
        ])

        report = analyze_test_script_input(events)
        self.assertEqual(report["status"], "pass")
        self.assertFalse(report["page_turn_measured"])

        result = self.run_input_check(events)
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("level=yes", result.stdout)
        self.assertIn("accumulation=no (not required", result.stdout)

    def test_vibration_and_accumulation_establish_the_harness(self) -> None:
        result = self.run_input_check(valid_test_script_input())

        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("status=pass", result.stdout)
        self.assertIn("vibration=yes", result.stdout)
        self.assertIn("level=yes", result.stdout)
        self.assertIn("accumulation=yes", result.stdout)

    def test_missing_or_false_production_level_read_is_not_established(self) -> None:
        base = [
            frame_input(
                1.0,
                1,
                held=True,
                edge_count=1,
                materialized_in_frame=0,
                frame_nr=1,
            ),
            frame_input(
                1.1,
                1,
                held=True,
                edge_count=0,
                materialized_in_frame=0,
                frame_nr=2,
            ),
        ]
        missing = base + [level_read(1.0, 1, held=True, frame_nr=1)]
        false = base + [
            level_read(1.0, 1, held=True, frame_nr=1),
            level_read(1.1, 1, held=False, frame_nr=2),
        ]

        for case, events in (("missing", missing), ("false", false)):
            with self.subTest(case=case):
                report = analyze_test_script_input(events)
                self.assertEqual(report["status"], "not-established")
                result = self.run_input_check(events)
                self.assertEqual(result.returncode, 1, result.stdout)
                self.assertIn("level=no", result.stdout)


class IdleHealthTests(unittest.TestCase):
    def test_powershell_harness_is_ascii_for_windows_powershell_51(self) -> None:
        # Windows PowerShell 5.1 treats UTF-8 without BOM as an ANSI code page. A multibyte
        # comment can swallow the following newline after mojibake, so keep this script ASCII.
        harness = Path(__file__).with_name("check-idle-health.ps1")
        source = harness.read_bytes().decode("ascii")
        self.assertIn("$CpuCoreRatio =", source)
        self.assertIn("GetForegroundWindow", source)

    def test_page_turn_harness_is_ascii_for_windows_powershell_51(self) -> None:
        harness = Path(__file__).with_name("page-turn-smoke.ps1")
        source = harness.read_bytes().decode("ascii")
        self.assertIn("--test-script", source)
        self.assertNotIn("MivSmokeInput", source)

    def test_empty_window_requires_external_sampler_confirmation_not_pid_alone(
        self,
    ) -> None:
        events = [session(42), frame(1.0, 1), tail(1.0, 1)]

        report = analyze_idle_health(events, 10.0, 25.0)

        self.assertEqual(report["status"], "fail")
        self.assertEqual(report["metrics"]["frames"], 0)
        self.assertEqual(report["metrics"]["update_rate_per_sec"], 0.0)

        pid_only_report = analyze_idle_health(
            events,
            10.0,
            25.0,
            expected_pid=42,
        )
        self.assertEqual(pid_only_report["status"], "fail")

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

    def test_video_pin_scenario_matches_target_work_case_insensitively(self) -> None:
        events = [
            session(42),
            frame(1.0, 1),
            tail(1.0, 1),
            thumb(0.75, "idle_upgrade_enqueue", "dir::c:/BOOKS/Video-Pin"),
        ]

        report = analyze_idle_health(
            events,
            1.0,
            10.0,
            expected_pid=42,
            require_work_key="C:/books/video-pin",
        )

        self.assertEqual(report["status"], "pass")
        self.assertGreater(report["setup_evidence"]["matched_events"], 0)
        self.assertEqual(
            report["setup_evidence"]["matched_kinds"],
            {"idle_upgrade_enqueue": 1},
        )
        self.assertEqual(report["setup_evidence"]["first_match_t"], 0.75)
        self.assertEqual(report["setup_evidence"]["last_match_t"], 0.75)
        self.assertEqual(report["setup_evidence"]["evidence_floor_t"], 0.0)
        self.assertEqual(
            report["setup_evidence"]["evidence_floor_basis"],
            "session_start",
        )
        self.assertEqual(report["warnings"], [])

    def test_video_pin_scenario_rejects_target_work_before_last_folder_load(
        self,
    ) -> None:
        events = [
            session(42),
            frame(1.0, 1),
            tail(1.0, 1),
            thumb(0.25, "idle_upgrade_ineligible", "dir::c:/books/video-pin"),
            thumb(0.75, "idle_upgrade_enqueue", "dir::c:/books/other"),
            load_folder_begin(0.5),
        ]

        report = analyze_idle_health(
            events,
            1.0,
            10.0,
            expected_pid=42,
            require_work_key="C:/books/video-pin",
        )

        self.assertEqual(report["status"], "fail")
        self.assertEqual(report["setup_evidence"]["matched_events"], 0)
        self.assertTrue(
            any("C:/books/video-pin" in item for item in report["failures"])
        )
        self.assertEqual(report["setup_evidence"]["evidence_floor_t"], 0.5)
        self.assertEqual(
            report["setup_evidence"]["evidence_floor_basis"],
            "last_load_folder_begin",
        )

    def test_video_pin_work_after_last_folder_load_passes_outside_measurement_window(
        self,
    ) -> None:
        events = [
            session(42),
            load_folder_begin(2.0),
            thumb(3.0, "idle_upgrade_ineligible", "dir::c:/books/video-pin"),
            frame(5.0, 1),
            tail(5.0, 1),
        ]

        report = analyze_idle_health(
            events,
            5.0,
            10.0,
            expected_pid=42,
            require_work_key="C:/books/video-pin",
        )

        self.assertEqual(report["status"], "pass")
        self.assertEqual(report["setup_evidence"]["matched_events"], 1)
        self.assertEqual(report["setup_evidence"]["first_match_t"], 3.0)
        self.assertEqual(report["setup_evidence"]["evidence_floor_t"], 2.0)

    def test_video_pin_scenario_warns_when_idle_upgrade_did_not_evaluate_tile(
        self,
    ) -> None:
        events = [
            session(42),
            frame(1.0, 1),
            tail(1.0, 1),
            thumb(0.75, "enqueue", "dir::c:/books/video-pin"),
        ]

        report = analyze_idle_health(
            events,
            1.0,
            10.0,
            expected_pid=42,
            require_work_key="C:/books/video-pin",
        )

        self.assertEqual(report["status"], "pass")
        self.assertEqual(report["setup_evidence"]["matched_events"], 1)
        self.assertEqual(len(report["warnings"]), 1)
        self.assertIn("from_cache=false", report["warnings"][0])

    def test_video_pin_work_key_gate_is_disabled_by_default(self) -> None:
        events = [session(42), frame(1.0, 1), tail(1.0, 1)]

        report = analyze_idle_health(
            events,
            1.0,
            10.0,
            expected_pid=42,
            require_work_key=None,
        )

        self.assertEqual(report["status"], "pass")
        self.assertEqual(report["thresholds"]["require_work_key"], None)
        self.assertEqual(report["setup_evidence"]["matched_events"], 0)

    def test_blank_work_key_is_rejected_instead_of_matching_everything(self) -> None:
        # "" は全 key に部分一致するのでゲートが常に通ってしまう。無効化は None で表す。
        events = [session(42), frame(1.0, 1), tail(1.0, 1)]

        for blank in ("", "   "):
            with self.assertRaises(ValueError):
                analyze_idle_health(
                    events,
                    1.0,
                    10.0,
                    expected_pid=42,
                    require_work_key=blank,
                )

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
        with writable_test_directory() as temp_dir:
            report_path = temp_dir / "idle-health.json"
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


class ThumbAdjustmentFrameTests(unittest.TestCase):
    def test_visible_totals_exclude_prefetch_and_keep_sessions_separate(self) -> None:
        events = [
            session(10),
            {"cat": "thumb", "kind": "adjustment_build", "origin": "visible", "n": 7,
             "width": 256, "height": 256, "apply_ms": 2.0, "texture_ms": 1.0, "total_ms": 3.0},
            {"cat": "thumb", "kind": "adjustment_build", "origin": "visible", "n": 7,
             "width": 256, "height": 256, "apply_ms": 4.0, "texture_ms": 1.0, "total_ms": 5.0},
            {"cat": "thumb", "kind": "adjustment_build", "origin": "prefetch", "n": 7,
             "width": 256, "height": 256, "total_ms": 100.0},
            session(11),
            {"cat": "thumb", "kind": "adjustment_build", "origin": "visible", "n": 7,
             "width": 256, "height": 256, "total_ms": 2.0},
        ]
        report = analyze_thumb_adjustment_frames(events)
        self.assertEqual(set(report["frames"]), {(1, 7), (2, 7)})
        self.assertEqual(sum(row["total_ms"] for row in report["frames"][(1, 7)]), 8.0)
        self.assertEqual(len(report["prefetch"]), 1)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            cmd_thumbs(events)
        self.assertIn("session=1 n=7 cells=2", out.getvalue())
        self.assertIn("total=8.0ms", out.getvalue())
        self.assertIn("prefetch=1 件", out.getvalue())


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


class HitchReportTests(unittest.TestCase):
    def report(self, events: list[dict], threshold_ms: float = 100.0) -> str:
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            cmd_hitches(events, threshold_ms)
        return output.getvalue()

    def test_new_schema_jsonl_preserves_deferred_interval_evidence(self) -> None:
        rows = [
            frame(1.0, 1843),
            {"t": 1.84, "cat": "ui", "kind": "update_breakdown", "n": 1843,
             "total_ms": 840.0, "total_cycles": 14000000,
             "other_worker_polls_ms": 810.0, "other_worker_polls_cycles": 1000000},
            frame(1.86, 1844),
            {"t": 2.0, "cat": "thumb", "kind": "load_phases", "tid": 22, "idx": 7,
             "key": "テスト.png", "start_t": 1.02, "end_t": 1.83,
             "total_ms": 810.0, "normal_log_ms": 790.0, "normal_log_cycles": 1000},
            # These rows are appended later, but describe earlier measured spans.
            {"t": 1.82, "cat": "log", "kind": "slow_io", "logger": "normal", "tid": 11,
             "start_t": 1.03, "acquired_t": 1.81, "end_t": 1.82,
             "operation": "log", "call_site": "global_search_ui.rs:456",
             "wait_ms": 780.0, "hold_ms": 10.0, "write_ms": 8.0, "flush_ms": 2.0,
             "holder_tid": 22, "holder_site": "thumb_loader.rs:123",
             "holder_snapshot": "at_wait_start_best_effort"},
            {"t": 1.83, "cat": "ui", "kind": "other_worker_polls_breakdown", "n": 1843,
             "start_t": 1.02, "end_t": 1.83, "total_ms": 810.0, "total_cycles": 1000000,
             "prepared_adoption_ms": 790.0, "prepared_adoption_cycles": 1000},
        ]
        contents = "\n".join(json.dumps(row, ensure_ascii=False) for row in rows) + "\n"
        with mock.patch.object(Path, "open", return_value=io.StringIO(contents)) as opened:
            events = load_events(Path("new-schema-deferred.jsonl"))
        opened.assert_called_once_with("r", encoding="utf-8")
        self.assertEqual([e["_line"] for e in events], list(range(1, 7)))
        self.assertEqual(events[4]["acquired_t"], 1.81)
        report = self.report(events)
        for expected in [
            "n=1843 [1.000, 1.840]s",
            "other_worker_polls [1.020, 1.830]s",
            "prepared_adoption=790.0ms cycles=1000",
            "[1.030, 1.820]s logger=normal tid=11",
            "wait=780.0ms", "holder(snapshot)=22/thumb_loader.rs:123",
            "[1.020, 1.830]s tid=22 idx=7 key=テスト.png",
            "normal_log=790.0ms cycles=1000",
        ]:
            self.assertIn(expected, report)

    def test_simultaneous_stall_shows_wait_holder_ui_and_thumb_evidence(self) -> None:
        events = [
            frame(1.0, 1843), frame(1.86, 1844),
            {"t": 1.844, "cat": "ui", "kind": "update_breakdown", "n": 1843,
             "total_ms": 844.0, "total_cycles": 14000000,
             "other_worker_polls_ms": 817.6, "other_worker_polls_cycles": 12000000},
            {"t": 1.83, "start_t": 1.01, "end_t": 1.8276, "cat": "ui",
             "kind": "other_worker_polls_breakdown", "n": 1843,
             "total_ms": 817.6, "total_cycles": 12000000,
             "details_meta_ms": 1.0, "details_meta_cycles": 1000000,
             "global_search_events_ms": 2.0, "global_search_events_cycles": 1000000,
             "prepared_adoption_ms": 800.0, "prepared_adoption_cycles": 1000000,
             "tag_prewarm_ms": 10.0, "tag_prewarm_cycles": 1000000,
             "video_pin_fetch_ms": 1.0, "video_pin_fetch_cycles": 1000000,
             "search_debounce_ms": 0.1, "other_ms": 3.5},
            {"t": 1.85, "cat": "thumb", "kind": "load_phases", "idx": 7, "tid": 22,
             "key": "test.png", "total_ms": 840.0, "total_cycles": 30000000,
             "decode_ms": 30.0, "decode_cycles": 28000000,
             "offer_raster_ms": 2.0, "offer_raster_cycles": 1000,
             "prefill_db_ms": 1.0, "prefill_db_cycles": 500,
             "stats_ms": 1.0, "stats_cycles": 1000,
             "normal_log_ms": 801.0, "normal_log_cycles": 2000,
             "perf_log_ms": 1.0, "perf_log_cycles": 500,
             "unaccounted_ms": 6.0},
            # Serialized last, but its actual end lies before several earlier rows.
            {"t": 1.82, "observed_t": 2.4, "cat": "log", "kind": "slow_io",
             "logger": "normal", "tid": 11, "start_t": 1.02, "end_t": 1.82,
             "wait_ms": 790.0, "hold_ms": 10.0, "write_ms": 8.0, "flush_ms": 2.0,
             "auxiliary_ms": 0.5, "acquired_t": 1.81, "operation": "log",
             "holder_tid": 22, "holder_site": "src/thumb_loader.rs:123",
             "call_site": "src/global_search_ui.rs:456"},
        ]
        report = self.report(events)
        for expected in [
            "n=1843 [1.000, 1.844]s", "other_worker_polls=817.6ms cycles=12000000",
            "other_worker_polls [1.010, 1.828]s", "prepared_adoption=800.0ms",
            "tag_prewarm=10.0ms", "video_pin_fetch=1.0ms", "UI 段は排他的計測",
            "logger=normal tid=11", "holder(snapshot)=22/src/thumb_loader.rs:123",
            "site=src/global_search_ui.rs:456", "wait=790.0ms", "write=8.0ms",
            "flush=2.0ms", "thumb.load_phases 重複区間: 1 件", "idx=7 key=test.png",
            "auxiliary=0.5ms", "acquired_t=1.81", "operation=log", "perf_log=1.0ms cycles=500",
            "normal_log=801.0ms cycles=2000", "offer_raster=2.0ms", "stats=1.0ms",
            "prefill_db=1.0ms", "prefill_db は offer_raster の内訳",
            "全カテゴリイベント時刻の空白", "OS paging・プロセス停止・logger 待ちを断定できない",
        ]:
            self.assertIn(expected, report)
        self.assertEqual(report, self.report(list(reversed(events))))

    def test_overlap_uses_intervals_including_completion_after_hitch(self) -> None:
        events = [frame(1.0, 1), frame(1.8, 2)]
        for site, start, end in [
            ("overlapping", 1.4, 2.1), ("before", 0.1, 1.0), ("after", 1.8, 2.2),
        ]:
            events.append({"t": end, "cat": "log", "kind": "slow_io", "logger": "perf",
                           "start_t": start, "end_t": end, "call_site": site,
                           "wait_ms": 500.0, "hold_ms": 200.0,
                           "holder_tid": None, "holder_site": None})
        events += [
            {"t": 2.2, "cat": "thumb", "kind": "load_phases", "total_ms": 900.0,
             "key": "overlapping-thumb"},
            {"t": 3.0, "cat": "thumb", "kind": "load_phases", "total_ms": 100.0,
             "key": "after-thumb"},
        ]
        report = self.report(events)
        self.assertIn("logger 重複区間: 1 件", report)
        self.assertIn("logger=perf", report)
        self.assertIn("site=overlapping", report)
        self.assertIn("holder(snapshot)=None/None", report)
        self.assertNotIn("site=before", report)
        self.assertNotIn("site=after", report)
        self.assertIn("thumb.load_phases 重複区間: 1 件", report)
        self.assertIn("key=overlapping-thumb", report)
        self.assertNotIn("key=after-thumb", report)

    def test_old_log_reports_previous_update_number_and_outside_time(self) -> None:
        events = [frame(2.0, 9), {**frame(3.0, 10), "prev_update_ms": 200.0,
                                "prev_update_cycles": 1000, "prev_outside_ms": 800.0},
                  {"t": 2.9, "cat": "nav", "kind": "apply_end", "ms": 3.0}]
        report = self.report(events)
        self.assertIn("n=9 [2.000, 2.200]s", report)
        self.assertIn("outside=800.0ms", report)
        self.assertIn("other_worker_polls 内訳なし", report)
        self.assertIn("apply_end=3.0ms", report)
        self.assertIn("cycles=1000", report)
        self.assertIn("区間と logger/thumb 重複判定は概算 (frame.begin 起点)", report)
        self.assertNotIn("n=10 [", report)

    def test_outer_endpoints_capture_leading_logger_stall_and_exclude_next_frame(self) -> None:
        events = [
            frame(1.4, 41),
            {"t": 1.43, "cat": "ui", "kind": "update_breakdown", "n": 41,
             "total_ms": 20.0, "grid_ms": 19.0},
            {**frame(2.0, 42), "prev_update_ms": 820.0,
             "prev_update_start_t": 1.0, "prev_update_end_t": 1.82},
            {"t": 1.39, "cat": "log", "kind": "slow_io", "logger": "perf",
             "start_t": 1.0, "end_t": 1.39, "call_site": "leading-stall",
             "wait_ms": 390.0},
            {"t": 2.1, "cat": "log", "kind": "slow_io", "logger": "perf",
             "start_t": 2.0, "end_t": 2.1, "call_site": "next-frame",
             "wait_ms": 100.0},
        ]
        report = self.report(events, 500.0)
        updates = report.split("\nupdate >= 500.0ms:", 1)[1].split("\n全カテゴリ", 1)[0]
        self.assertIn("n=41 [1.000, 1.820]s total=820.0ms", updates)
        self.assertIn("inner update_frame [1.410, 1.430]s total=20.0ms", updates)
        self.assertIn("logger 重複区間: 1 件", updates)
        self.assertIn("site=leading-stall", updates)
        self.assertNotIn("site=next-frame", updates)
        self.assertNotIn("概算", updates)
        self.assertNotIn("n=42 [", updates)
        self.assertEqual(report, self.report(list(reversed(events)), 500.0))

    def test_missing_or_malformed_outer_endpoints_keep_approximate_legacy_span(self) -> None:
        for start, end in [(None, 1.82), (1.0, None), ("invalid", 1.82),
                           (1.82, 1.0), (float("nan"), 1.82)]:
            with self.subTest(start=start, end=end):
                report = self.report([
                    frame(1.4, 41),
                    {**frame(2.3, 42), "prev_update_ms": 820.0,
                     "prev_update_start_t": start, "prev_update_end_t": end},
                ], 500.0)
                self.assertIn("n=41 [1.400, 2.220]s total=820.0ms", report)
                self.assertIn("区間と logger/thumb 重複判定は概算 (frame.begin 起点)", report)
                self.assertNotIn("n=42 [", report)

    def test_outer_and_inner_measurements_coexist_by_previous_frame_number(self) -> None:
        events = [
            frame(1.0, 41),
            {"t": 1.6, "cat": "ui", "kind": "update_breakdown", "n": 41,
             "total_ms": 600.0, "total_cycles": 6000, "grid_ms": 590.0},
            {**frame(1.9, 42), "prev_update_ms": 820.0,
             "prev_update_cycles": 8200, "prev_outside_ms": 80.0},
            {"t": 2.5, "cat": "ui", "kind": "update_breakdown", "n": 42,
             "total_ms": 600.0, "total_cycles": 4200, "grid_ms": 599.0},
        ]
        report = self.report(events, 500.0)
        self.assertIn("update >= 500.0ms: 2 件", report)
        self.assertIn("n=41 [1.000, 1.820]s total=820.0ms cycles=8200", report)
        self.assertIn("inner update_frame [1.000, 1.600]s total=600.0ms cycles=6000", report)
        self.assertIn("grid=590.0ms", report)
        self.assertIn("outside=80.0ms", report)
        self.assertIn("n=42 [1.900, 2.500]s total=600.0ms cycles=4200", report)
        self.assertNotIn("n=42 [1.000, 1.820]s", report)
        self.assertEqual(report, self.report(list(reversed(events)), 500.0))

    def test_short_inner_breakdown_keeps_outer_hitch_and_end_of_frame_logger_stall(self) -> None:
        events = [
            frame(1.0, 41),
            {"t": 1.02, "cat": "ui", "kind": "update_breakdown", "n": 41,
             "total_ms": 20.0, "grid_ms": 19.0},
            {**frame(1.9, 42), "prev_update_ms": 820.0, "prev_update_cycles": 8200},
            {"t": 1.91, "cat": "ui", "kind": "update_breakdown", "n": 42,
             "total_ms": 10.0, "grid_ms": 9.0},
            # Deferred logger record describes work after the inner breakdown.
            {"t": 1.82, "cat": "log", "kind": "slow_io", "logger": "perf",
             "start_t": 1.02, "end_t": 1.82, "call_site": "end-of-frame",
             "wait_ms": 800.0},
        ]
        report = self.report(events, 500.0)
        updates = report.split("\nupdate >= 500.0ms:", 1)[1].split("\n全カテゴリ", 1)[0]
        self.assertIn("1 件", updates)
        self.assertIn("n=41 [1.000, 1.820]s total=820.0ms", updates)
        self.assertIn("inner update_frame [1.000, 1.020]s total=20.0ms", updates)
        self.assertIn("grid=19.0ms", updates)
        self.assertIn("logger 重複区間: 1 件", updates)
        self.assertIn("site=end-of-frame", updates)
        self.assertNotIn("n=42", updates)
        self.assertEqual(report, self.report(list(reversed(events)), 500.0))

    def test_inner_only_last_frame_remains_partial_without_outer_measurement(self) -> None:
        events = [
            frame(1.0, 41),
            {"t": 1.6, "cat": "ui", "kind": "update_breakdown", "n": 41,
             "total_ms": 600.0, "grid_ms": 590.0},
        ]
        report = self.report(events, 500.0)
        self.assertIn("update >= 500.0ms: 1 件", report)
        self.assertIn("n=41 [1.000, 1.600]s total=600.0ms", report)
        self.assertIn("inner update_frame のみ (部分計測; outer 未計測)", report)
        self.assertIn("grid=590.0ms", report)

    def test_missing_begin_or_inner_endpoint_does_not_invent_outer_interval(self) -> None:
        # A current begin cannot identify the absent previous frame's start or n.
        events = [
            {**frame(1.9, 42), "prev_update_ms": 820.0},
            {"cat": "ui", "kind": "update_breakdown", "n": 41,
             "total_ms": 600.0, "grid_ms": 590.0},
        ]
        report = self.report(events, 500.0)
        self.assertIn("update >= 500.0ms: 1 件", report)
        self.assertIn("n=41 [区間不明] total=600.0ms", report)
        self.assertIn("inner update_frame のみ (部分計測; outer 未計測)", report)
        self.assertNotIn("total=820.0ms", report)
        self.assertNotIn("n=42 [", report)
        events[1]["total_ms"] = 20.0
        self.assertIn("update >= 500.0ms: 0 件", self.report(events, 500.0))

    def test_update_number_wins_over_adjacent_timestamp(self) -> None:
        events = [
            {"t": 5.0, "cat": "ui", "kind": "update_breakdown", "n": 10, "total_ms": 800.0},
            {"t": 5.0, "cat": "ui", "kind": "other_worker_polls_breakdown", "n": 11,
             "total_ms": 700.0, "prepared_adoption_ms": 699.0},
            {"t": 10.0, "cat": "ui", "kind": "other_worker_polls_breakdown", "n": 10,
             "start_t": 4.2, "end_t": 4.9, "total_ms": 700.0, "details_meta_ms": 699.0},
        ]
        report = self.report(events)
        self.assertIn("frame.begin が 2 件未満", report)
        self.assertIn("n=10 [4.200, 5.000]s", report)
        self.assertIn("details_meta=699.0ms", report)
        self.assertNotIn("prepared_adoption=699.0ms", report)

    def test_log_gaps_use_all_categories_sorted_not_only_frames(self) -> None:
        events = [frame(1.8, 2), {"t": 1.05, "cat": "thumb", "kind": "decode"},
                  frame(1.0, 1), {"t": 1.85, "cat": "search", "kind": "done"}]
        report = self.report(events)
        self.assertIn("全カテゴリイベント時刻の空白 >= 100.0ms: 1 件", report)
        self.assertIn("[1.050, 1.800]s gap=750.0ms", report)
        self.assertIn("update 計測なし", report)
        self.assertNotIn("gap=-", report)

    def test_empty_and_missing_optional_diagnostics_are_supported(self) -> None:
        self.assertIn("フレーム数: 0", self.report([]))
        report = self.report([
            {"t": 1.0, "cat": "ui", "kind": "update_breakdown", "n": 1, "total_ms": 150.0},
            {"t": 0.99, "cat": "thumb", "kind": "load_phases", "total_ms": 140.0,
             "decode_ms": 20.0, "decode_parts": {"orientation_ms": 5.0}},
        ])
        self.assertIn("total=150.0ms cycles=n/a", report)
        self.assertIn("decode=20.0ms cycles=n/a", report)
        self.assertIn("other_worker_polls 内訳なし", report)

    def test_dropped_diagnostics_are_reported_as_incomplete_evidence(self) -> None:
        report = self.report([
            {"t": 1.0, "cat": "log", "kind": "diagnostic_dropped", "count": 2},
            {"t": 2.0, "cat": "log", "kind": "diagnostic_dropped", "count": 3},
        ])
        self.assertIn("logger 診断欠落: 5 件", report)
        self.assertIn("queue 競合/飽和等", report)
        self.assertIn("待ち先の証拠は不完全", report)

    def test_thumb_total_cycles_do_not_fabricate_candidate_cycles(self) -> None:
        report = self.report([
            frame(1.0, 41), frame(1.9, 42),
            {"t": 1.85, "cat": "thumb", "kind": "load_phases",
             "start_t": 1.01, "end_t": 1.85, "total_ms": 840.0,
             "total_cycles": 30000000, "offer_raster_ms": 2.0, "stats_ms": 1.0,
             "normal_log_ms": 801.0, "perf_log_ms": 1.0, "prefill_db_ms": 0.5},
        ])
        self.assertIn("total=840.0ms cycles=30000000", report)
        for candidate, ms in [
            ("offer_raster", 2.0), ("stats", 1.0), ("normal_log", 801.0),
            ("perf_log", 1.0), ("prefill_db", 0.5),
        ]:
            self.assertIn(f"{candidate}={ms:.1f}ms cycles=n/a", report)
        self.assertNotIn("cycles=0", report)


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


class MemoryTimelineTests(unittest.TestCase):
    def test_session_anchor_maps_samples_before_and_after_late_perf_init_to_utc(self):
        events = [
            {"t": 7.0, "cat": "session", "kind": "start", "pid": 42,
             "wall_unix_ms": 1_700_000_000_123, "wall_t": 5.25,
             "process_start_unix_ms": 1_699_999_990_000},
            {"t": 5.5, "cat": "process_memory", "kind": "sample", "stage": "later", "pid": 42},
            {"t": 1.0, "cat": "process_memory", "kind": "milestone", "stage": "earlier", "pid": 42},
        ]
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            cmd_memory(events)
        report = output.getvalue()
        self.assertIn("session pid=42 wall=2023-11-14T22:13:20.123Z at t=5.250000s", report)
        self.assertIn("core process creation (UTC): 2023-11-14T22:13:10.000Z", report)
        self.assertRegex(report, r"2023-11-14T22:13:15\.873Z\s+1\.000.*milestone / earlier")
        self.assertRegex(report, r"2023-11-14T22:13:20\.373Z\s+5\.500.*sample / later")

    def test_legacy_session_header_still_prints_relative_samples(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            cmd_memory([
                {"t": 5.0, "cat": "session", "kind": "start", "pid": 42},
                {"t": 6.0, "cat": "process_memory", "kind": "sample", "stage": "legacy", "pid": 42},
            ])
        report = output.getvalue()
        self.assertIn("UTC 対応情報なし", report)
        self.assertRegex(report, r"6\.000\s+42.*sample / legacy")

    def test_timeline_sorts_samples_converts_bytes_and_keeps_stages_and_pid(self):
        samples = [
            {"t": 2.0, "cat": "process_memory", "kind": "end", "stage": "ai_runtime_init", "pid": 42,
             "private_bytes": 5 * 1024 * 1024, "working_set_bytes": 3 * 1024 * 1024,
             "peak_working_set_bytes": 4 * 1024 * 1024, "pagefile_bytes": 6 * 1024 * 1024},
            {"t": 1.0, "cat": "process_memory", "kind": "sample", "stage": "startup_sampler", "pid": 42},
            {"t": 0.0, "cat": "startup", "kind": "settings_load"},
            {"t": 3.0, "cat": "process_memory", "kind": "query_failed", "stage": "pdf_pool_spawn", "error": 5},
        ]
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            cmd_memory(samples)
        report = output.getvalue()
        self.assertLess(report.index("startup_sampler"), report.index("ai_runtime_init"))
        self.assertIn("42", report)
        self.assertRegex(report, r"5\.00\s+3\.00\s+4\.00\s+6\.00\s+end / ai_runtime_init")
        self.assertIn("Win32 error=5", report)
        self.assertNotIn("settings_load", report)

    def test_old_logs_and_memory_cli(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            cmd_memory([{"cat": "startup", "kind": "first_frame"}])
        self.assertIn("process_memory イベントなし", output.getvalue())
        with mock.patch.object(sys, "argv", ["analyze_perf.py", "unused.jsonl", "memory"]), \
                mock.patch.object(Path, "is_file", return_value=True), \
                mock.patch("analyze_perf.load_events", return_value=[]), \
                contextlib.redirect_stdout(io.StringIO()), \
                mock.patch("analyze_perf.cmd_memory") as command:
            main()
        command.assert_called_once_with([])


if __name__ == "__main__":
    unittest.main()
