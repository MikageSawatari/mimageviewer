#!/usr/bin/env python3
"""Focused false-PASS regressions for the isolated §1.198 analyzer."""

from __future__ import annotations

import copy
import unittest

from analyze_idle198_convergence import analyze_idle198_convergence


PID = 4242
GENERATION = 19
KEYS = [rf"C:\fixture\idle198-{number:03d}.png" for number in range(1, 5)]


def step(t: float, message: str) -> dict:
    return {"t": t, "cat": "test_script", "kind": "step", "message": message}


def load(
    t: float,
    key: str,
    idx: int,
    evaluated: int,
    width: int,
    height: int,
    *,
    skip_cache: bool,
) -> dict:
    return {
        "t": t,
        "cat": "thumb",
        "kind": "load_phases",
        "key": key,
        "idx": idx,
        "seq": 7,
        "input_seq": 7,
        "skip_cache": skip_cache,
        "evaluated_display_px": evaluated,
        "display_output_width": width,
        "display_output_height": height,
        "should_save": False,
    }


def enqueue(t: float, key: str, idx: int) -> dict:
    return {
        "t": t,
        "cat": "thumb",
        "kind": "idle_upgrade_enqueue",
        "key": key,
        "idx": idx,
        "seq": 7,
        "items_gen": GENERATION,
        "skip_cache": True,
    }


def valid_events() -> list[dict]:
    events = [
        {"t": 0.0, "cat": "session", "kind": "start", "pid": PID},
        step(
            1.0,
            "idle198:root is_fullscreen=false items_len=4 pending_thumbs=0 "
            "target_mode=targeted target_current=true "
            "context_serial=1 viewport=ROOT "
            "host_incarnation=2 hwnd=3 backend_token=4 "
            f"items_generation={GENERATION}",
        ),
        step(2.0, "idle198:small:begin"),
    ]
    for idx, key in enumerate(KEYS):
        events.append(load(3.0 + idx * 0.1, key, idx, 300, 247, 124, skip_cache=False))
    events.append(step(10.0, "idle198:374:begin"))
    for idx, key in enumerate(KEYS):
        events.append(enqueue(10.5 + idx * 0.1, key, idx))
        events.append(load(11.0 + idx * 0.1, key, idx, 374, 247, 124, skip_cache=True))
    events.extend(
        [
            step(35.0, "idle198:374:end"),
            step(36.0, "idle198:enlarged:begin"),
        ]
    )
    for idx, key in enumerate(KEYS):
        events.append(enqueue(36.5 + idx * 0.1, key, idx))
        events.append(load(37.0 + idx * 0.1, key, idx, 438, 438, 220, skip_cache=True))
    events.extend(
        [
            step(61.0, "idle198:enlarged:end"),
            step(
                62.0,
                "idle198:final is_fullscreen=false items_len=4 pending_thumbs=0 "
                "target_mode=targeted target_current=true same_root=true "
                f"items_generation={GENERATION}",
            ),
            step(63.0, "idle198:done"),
        ]
    )
    return sorted(events, key=lambda event: event["t"])


def valid_samples() -> list[dict]:
    samples = []
    marker_thresholds = [
        (2_000, "idle198:small:begin"),
        (10_000, "idle198:374:begin"),
        (35_000, "idle198:374:end"),
        (36_000, "idle198:enlarged:begin"),
        (61_000, "idle198:enlarged:end"),
    ]
    for elapsed_ms in range(0, 64_001, 500):
        observed = [
            marker for threshold, marker in marker_thresholds if elapsed_ms >= threshold
        ]
        samples.append(
            {
                "schema_version": 2,
                "sample_index": len(samples),
                "elapsed_ms": elapsed_ms,
                "phase": (
                    "final-marker-observed"
                    if "idle198:enlarged:end" in observed
                    else ("focus-acquired" if elapsed_ms == 0 else "running")
                ),
                "app_pid": PID,
                "process_start_utc": "2026-09-08T00:00:00.0000000Z",
                "pre_alive": True,
                "pre_foreground_pid": PID,
                "post_alive": True,
                "post_foreground_pid": PID,
                "alive": True,
                "foreground_pid": PID,
                "matches_expected_process": True,
                "observed_markers": observed,
            }
        )
    return samples


class Idle198ConvergenceAnalyzerTests(unittest.TestCase):
    def analyze(self, events=None, samples=None):
        return analyze_idle198_convergence(
            valid_events() if events is None else events,
            KEYS,
            PID,
            valid_samples() if samples is None else samples,
        )

    def test_correlates_each_generation_and_accepts_empty_quiet_intervals(self):
        report = self.analyze()
        self.assertEqual("pass", report["status"], report["failures"])
        self.assertEqual(0, report["quiet_windows"]["quality374"]["quiet_fixture_work_count"])
        self.assertEqual(0, report["quiet_windows"]["enlarged"]["quiet_fixture_work_count"])
        self.assertGreaterEqual(
            report["quiet_windows"]["quality374"]["quiet_duration_secs"], 15.0
        )

    def test_same_millisecond_adjacent_markers_use_log_order(self):
        events = valid_events()
        enlarged_end = next(
            event
            for event in events
            if event.get("message") == "idle198:enlarged:end"
        )
        final = next(
            event
            for event in events
            if str(event.get("message", "")).startswith("idle198:final ")
        )
        done = next(
            event for event in events if event.get("message") == "idle198:done"
        )
        final["t"] = enlarged_end["t"]
        done["t"] = enlarged_end["t"]
        report = self.analyze(events=events)
        self.assertEqual("pass", report["status"], report["failures"])

        events.remove(done)
        enlarged_end_index = events.index(enlarged_end)
        events.insert(enlarged_end_index, done)
        report = self.analyze(events=events)
        self.assertEqual("fail", report["status"])
        self.assertTrue(any("perf log" in value for value in report["failures"]))

    def test_omitted_perf_sequence_is_the_documented_zero_sequence(self):
        events = valid_events()
        for event in events:
            if event.get("cat") != "thumb":
                continue
            event.pop("seq", None)
            if event.get("kind") == "load_phases":
                event["input_seq"] = 0
        report = self.analyze(events=events)
        self.assertEqual("pass", report["status"], report["failures"])

    def test_repeated_work_inside_quiet_interval_is_not_silence(self):
        events = valid_events()
        events.append(enqueue(20.0, KEYS[0], 0))
        report = self.analyze(events=events)
        self.assertEqual("fail", report["status"])
        self.assertTrue(any("quiet window" in value for value in report["failures"]))

    def test_other_pid_sample_cannot_authorize_sleeping_window(self):
        samples = valid_samples()
        samples[5]["post_foreground_pid"] = PID + 1
        samples[5]["foreground_pid"] = PID + 1
        samples[5]["matches_expected_process"] = False
        report = self.analyze(samples=samples)
        self.assertEqual("fail", report["status"])
        self.assertTrue(any("foreground process" in value for value in report["failures"]))

    def test_window_loss_after_valid_final_sample_does_not_erase_proof(self):
        samples = valid_samples()
        samples[-1]["post_foreground_pid"] = PID + 1
        samples[-1]["foreground_pid"] = PID + 1
        samples[-1]["matches_expected_process"] = False
        samples[-1]["elapsed_ms"] += 2_000
        samples[-1]["observed_markers"] = []
        report = self.analyze(samples=samples)
        self.assertEqual("pass", report["status"], report["failures"])
        self.assertGreater(report["lifetime_samples"]["post_proof_count"], 0)
        self.assertEqual(
            1, report["lifetime_samples"]["post_proof_invalid_count"]
        )

    def test_invalid_first_final_sample_is_not_rescued_by_a_later_sample(self):
        samples = valid_samples()
        first_final = next(
            sample
            for sample in samples
            if sample["elapsed_ms"] == 61_000
        )
        first_final["post_alive"] = False
        first_final["alive"] = False
        first_final["matches_expected_process"] = False
        report = self.analyze(samples=samples)
        self.assertEqual("fail", report["status"])
        self.assertTrue(any("proof samples" in value for value in report["failures"]))

    def test_wrong_374_request_cannot_pass_calibration(self):
        events = valid_events()
        target = next(
            event
            for event in events
            if event.get("kind") == "load_phases"
            and event.get("key") == KEYS[0]
            and event.get("evaluated_display_px") == 374
        )
        target["evaluated_display_px"] = 375
        report = self.analyze(events=events)
        self.assertEqual("fail", report["status"])
        self.assertTrue(any("quality374" in value for value in report["failures"]))

    def test_wrong_374_output_dimensions_cannot_pass(self):
        events = valid_events()
        target = next(
            event
            for event in events
            if event.get("kind") == "load_phases"
            and event.get("key") == KEYS[0]
            and event.get("evaluated_display_px") == 374
        )
        target["display_output_width"] = 246
        report = self.analyze(events=events)
        self.assertEqual("fail", report["status"])
        self.assertTrue(any("expected 247x124" in value for value in report["failures"]))

    def test_missing_source_generation_cannot_pass_from_later_silence(self):
        events = [
            event
            for event in valid_events()
            if not (
                event.get("kind") == "load_phases"
                and event.get("key") == KEYS[2]
                and event.get("evaluated_display_px") == 374
            )
        ]
        report = self.analyze(events=events)
        self.assertEqual("fail", report["status"])
        self.assertTrue(any("completion count" in value for value in report["failures"]))

    def test_same_key_with_wrong_index_is_not_correlated(self):
        events = copy.deepcopy(valid_events())
        target = next(
            event
            for event in events
            if event.get("kind") == "load_phases"
            and event.get("key") == KEYS[1]
            and event.get("evaluated_display_px") == 374
        )
        target["idx"] = 99
        report = self.analyze(events=events)
        self.assertEqual("fail", report["status"])
        self.assertTrue(any("idx mismatch" in value for value in report["failures"]))

    def test_wrong_enqueue_generation_is_not_hidden_by_matching_load(self):
        events = valid_events()
        target = next(
            event
            for event in events
            if event.get("kind") == "idle_upgrade_enqueue"
            and event.get("key") == KEYS[3]
            and event["t"] < 35.0
        )
        target["items_gen"] = GENERATION + 1
        report = self.analyze(events=events)
        self.assertEqual("fail", report["status"])
        self.assertTrue(any("enqueue generation" in value for value in report["failures"]))

    def test_single_final_sample_cannot_stand_in_for_whole_interval_sampling(self):
        sample = valid_samples()[-1]
        sample["sample_index"] = 0
        report = self.analyze(samples=[sample])
        self.assertEqual("fail", report["status"])
        self.assertTrue(any("sampling started after" in value for value in report["failures"]))

    def test_missing_front_half_of_samples_cannot_pass(self):
        samples = valid_samples()[40:]
        for index, sample in enumerate(samples):
            sample["sample_index"] = index
        report = self.analyze(samples=samples)
        self.assertEqual("fail", report["status"])
        self.assertTrue(any("sampling started after" in value for value in report["failures"]))

    def test_duplicated_or_reversed_sample_sequence_is_rejected(self):
        samples = valid_samples()
        samples[8]["sample_index"] = samples[7]["sample_index"]
        samples[20]["elapsed_ms"] = samples[19]["elapsed_ms"] - 1
        report = self.analyze(samples=samples)
        self.assertEqual("fail", report["status"])
        self.assertTrue(any("indices" in value for value in report["failures"]))
        self.assertTrue(any("elapsed times" in value for value in report["failures"]))

    def test_cross_phase_input_sequence_change_is_rejected(self):
        events = valid_events()
        target = next(
            event
            for event in events
            if event.get("kind") == "load_phases"
            and event.get("key") == KEYS[0]
            and event.get("evaluated_display_px") == 438
        )
        target["input_seq"] = 8
        report = self.analyze(events=events)
        self.assertEqual("fail", report["status"])
        self.assertTrue(any("crossed input sequences" in value for value in report["failures"]))


if __name__ == "__main__":
    unittest.main()
