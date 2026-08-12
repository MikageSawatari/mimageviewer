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
    analyze_page_turn,
    analyze_test_script_input,
    analyze_idle_health,
    cmd_colorize,
    cmd_idle_health,
    cmd_page_turn,
    cmd_pre_grid,
    load_events,
    main,
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


def thumb(t: float, kind: str, key: str) -> dict:
    return {
        "t": t,
        "cat": "thumb",
        "kind": kind,
        "key": key,
        "idx": 7,
        "items_gen": 2,
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
) -> dict:
    return {
        "t": t,
        "cat": "fs",
        "kind": "page_turn_decision",
        "idx": idx,
        "items_generation": generation,
        "reason": reason,
        "passthrough_rendition_ready": passthrough_rendition_ready,
        "defer_ui_uploads": defer_ui_uploads,
    }


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
                + [page_turn_gate_decision(1.05, 2, defer_ui_uploads=False)],
                common_good
                + [page_turn_gate_decision(1.05, 2, defer_ui_uploads=True)],
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
            evidence_start_t=0.5,
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
        self.assertEqual(report["warnings"], [])

    def test_video_pin_scenario_rejects_other_or_out_of_window_work(self) -> None:
        events = [
            session(42),
            frame(1.0, 1),
            tail(1.0, 1),
            thumb(0.25, "idle_upgrade_ineligible", "dir::c:/books/video-pin"),
            thumb(0.75, "idle_upgrade_enqueue", "dir::c:/books/other"),
        ]

        report = analyze_idle_health(
            events,
            1.0,
            10.0,
            expected_pid=42,
            evidence_start_t=0.5,
            require_work_key="C:/books/video-pin",
        )

        self.assertEqual(report["status"], "fail")
        self.assertEqual(report["setup_evidence"]["matched_events"], 0)
        self.assertTrue(
            any("C:/books/video-pin" in item for item in report["failures"])
        )

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
            evidence_start_t=0.5,
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
            evidence_start_t=0.5,
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
