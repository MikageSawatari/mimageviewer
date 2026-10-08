#!/usr/bin/env python3
"""Correlate the isolated §1.198 idle-upgrade live scenario.

This is deliberately stricter than the generic idle-health command.  Silence
is accepted only after every fixture item has an ordered idle enqueue and
source completion for the requested quality, under one unchanged item
generation and with independent process/foreground samples.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path
from typing import Any, Iterable


PHASE_MARKERS = (
    "idle198:small:begin",
    "idle198:374:begin",
    "idle198:374:end",
    "idle198:enlarged:begin",
    "idle198:enlarged:end",
    "idle198:done",
)
BOUNDARY_EXCLUSION_SECS = 0.5
LIFETIME_MARKERS = PHASE_MARKERS[:-1]


def read_json_lines(path: Path) -> list[dict[str, Any]]:
    events: list[dict[str, Any]] = []
    with path.open("r", encoding="utf-8-sig") as stream:
        for line_number, line in enumerate(stream, 1):
            if not line.strip():
                continue
            try:
                value = json.loads(line)
            except json.JSONDecodeError as error:
                raise ValueError(f"{path}:{line_number}: invalid JSON: {error}") from error
            if not isinstance(value, dict):
                raise ValueError(f"{path}:{line_number}: expected a JSON object")
            events.append(value)
    return events


def normalize_key(value: object) -> str:
    return str(value).replace("/", "\\").casefold()


def event_time(event: dict[str, Any]) -> float:
    return float(event.get("t", 0.0))


def parse_fields(message: str, prefix: str) -> dict[str, str]:
    if not message.startswith(prefix):
        return {}
    fields: dict[str, str] = {}
    for part in message[len(prefix) :].strip().split():
        if "=" in part:
            name, value = part.split("=", 1)
            fields[name] = value
    return fields


def _one_marker(
    events: list[dict[str, Any]], message: str, failures: list[str]
) -> dict[str, Any] | None:
    matches = [
        event
        for event in events
        if event.get("cat") == "test_script"
        and event.get("kind") == "step"
        and event.get("message") == message
    ]
    if len(matches) != 1:
        failures.append(f"marker {message!r} count was {len(matches)}, expected 1")
        return None
    return matches[0]


def _one_prefixed_marker(
    events: list[dict[str, Any]], prefix: str, failures: list[str]
) -> dict[str, Any] | None:
    matches = [
        event
        for event in events
        if event.get("cat") == "test_script"
        and event.get("kind") == "step"
        and str(event.get("message", "")).startswith(prefix)
    ]
    if len(matches) != 1:
        failures.append(f"marker prefix {prefix!r} count was {len(matches)}, expected 1")
        return None
    return matches[0]


def _expected_event_key(
    event: dict[str, Any], expected_by_normalized: dict[str, str]
) -> str | None:
    return expected_by_normalized.get(normalize_key(event.get("key", "")))


def _positive_int(event: dict[str, Any], name: str) -> int | None:
    try:
        value = int(event[name])
    except (KeyError, TypeError, ValueError):
        return None
    return value if value > 0 else None


def _int_field(event: dict[str, Any], name: str) -> int | None:
    try:
        return int(event[name])
    except (KeyError, TypeError, ValueError):
        return None


def _perf_input_seq(event: dict[str, Any]) -> int | None:
    # perf::event omits its top-level seq when it is zero. load_phases also
    # carries the unambiguous input_seq extra, while enqueue uses that common
    # omitted-zero convention.
    if "input_seq" in event:
        return _int_field(event, "input_seq")
    if "seq" not in event:
        return 0
    return _int_field(event, "seq")


def _analyze_quality_phase(
    events: list[dict[str, Any]],
    expected_keys: list[str],
    expected_by_normalized: dict[str, str],
    generation: int,
    lower_t: float,
    begin_t: float,
    end_t: float,
    phase: str,
    failures: list[str],
) -> dict[str, Any]:
    enqueues_by_key: dict[str, list[dict[str, Any]]] = {
        key: [] for key in expected_keys
    }
    loads_by_key: dict[str, list[dict[str, Any]]] = {key: [] for key in expected_keys}
    wrong_loads_by_key: dict[str, list[dict[str, Any]]] = {
        key: [] for key in expected_keys
    }

    for event in events:
        t = event_time(event)
        if not lower_t <= t < end_t or event.get("cat") != "thumb":
            continue
        expected_key = _expected_event_key(event, expected_by_normalized)
        if expected_key is None:
            continue
        if event.get("kind") == "idle_upgrade_enqueue":
            try:
                event_generation = int(event.get("items_gen"))
            except (TypeError, ValueError):
                event_generation = -1
            if event_generation != generation:
                failures.append(
                    f"{phase} {expected_key}: idle enqueue generation "
                    f"{event_generation} != {generation}"
                )
            if event.get("skip_cache") is not True:
                failures.append(f"{phase} {expected_key}: idle enqueue did not bypass cache")
            enqueues_by_key[expected_key].append(event)
        elif event.get("kind") == "load_phases" and event.get("skip_cache") is True:
            evaluated = _int_field(event, "evaluated_display_px")
            if (phase == "quality374" and evaluated == 374) or (
                phase == "enlarged" and evaluated is not None and evaluated > 374
            ):
                loads_by_key[expected_key].append(event)
            else:
                wrong_loads_by_key[expected_key].append(event)

    completions: list[dict[str, Any]] = []
    details: dict[str, Any] = {}
    for key in expected_keys:
        enqueues = sorted(enqueues_by_key[key], key=event_time)
        loads = sorted(loads_by_key[key], key=event_time)
        wrong_loads = sorted(wrong_loads_by_key[key], key=event_time)
        if len(enqueues) != 1:
            failures.append(
                f"{phase} {key}: idle enqueue count was {len(enqueues)}, expected 1"
            )
        if len(loads) != 1:
            failures.append(
                f"{phase} {key}: matching source completion count was "
                f"{len(loads)}, expected 1"
            )
        if wrong_loads:
            observed = [event.get("evaluated_display_px") for event in wrong_loads]
            failures.append(
                f"{phase} {key}: unexpected skip-cache source requests {observed}"
            )
        if len(enqueues) != 1 or len(loads) != 1:
            continue
        enqueue = enqueues[0]
        load = loads[0]
        if event_time(load) < event_time(enqueue):
            failures.append(f"{phase} {key}: source completion preceded idle enqueue")
        enqueue_idx = _int_field(enqueue, "idx")
        load_idx = _int_field(load, "idx")
        if enqueue_idx is None or load_idx is None or enqueue_idx != load_idx:
            failures.append(
                f"{phase} {key}: enqueue/load idx mismatch "
                f"{enqueue.get('idx')} != {load.get('idx')}"
            )
        enqueue_seq = _perf_input_seq(enqueue)
        load_seq = _perf_input_seq(load)
        if enqueue_seq is None or load_seq is None or enqueue_seq != load_seq:
            failures.append(
                f"{phase} {key}: enqueue/load input sequence mismatch "
                f"{enqueue.get('seq')} != {load.get('input_seq')}"
            )
        width = _positive_int(load, "display_output_width")
        height = _positive_int(load, "display_output_height")
        if width is None or height is None:
            failures.append(f"{phase} {key}: source output dimensions are missing")
        elif phase == "quality374" and (width, height) != (247, 124):
            failures.append(
                f"quality374 {key}: output was {width}x{height}, expected 247x124"
            )
        elif phase == "enlarged" and max(width, height) <= 247:
            failures.append(
                f"enlarged {key}: output {width}x{height} did not exceed the 247px baseline"
            )
        completions.append(load)
        details[key] = {
            "idx": load.get("idx"),
            "input_seq": load_seq,
            "enqueue_t": event_time(enqueue),
            "completion_t": event_time(load),
            "evaluated_display_px": load.get("evaluated_display_px"),
            "display_output_width": width,
            "display_output_height": height,
            "should_save": load.get("should_save"),
        }

    quiet_start = (
        max([begin_t, *(event_time(event) for event in completions)])
        + BOUNDARY_EXCLUSION_SECS
    )
    quiet_end = end_t - BOUNDARY_EXCLUSION_SECS
    quiet_duration = quiet_end - quiet_start
    if quiet_duration < 15.0:
        failures.append(
            f"{phase}: post-completion quiet window was {quiet_duration:.3f}s, expected >=15s"
        )

    repeated = []
    for event in events:
        t = event_time(event)
        if not quiet_start <= t <= quiet_end or event.get("cat") != "thumb":
            continue
        if _expected_event_key(event, expected_by_normalized) is None:
            continue
        if event.get("kind") in {"idle_upgrade_enqueue", "load_phases"}:
            repeated.append(event)
    if repeated:
        failures.append(
            f"{phase}: {len(repeated)} fixture enqueue/load events occurred in the quiet window"
        )

    return {
        "lower_t": lower_t,
        "begin_t": begin_t,
        "end_t": end_t,
        "quiet_start_t": quiet_start,
        "quiet_end_t": quiet_end,
        "quiet_duration_secs": quiet_duration,
        "fixture": details,
        "quiet_fixture_work_count": len(repeated),
    }


def analyze_idle198_convergence(
    events: list[dict[str, Any]],
    expected_keys: Iterable[str],
    expected_pid: int,
    lifetime_samples: list[dict[str, Any]],
) -> dict[str, Any]:
    failures: list[str] = []
    environment_failures: list[str] = []
    expected_keys = list(expected_keys)
    expected_by_normalized = {normalize_key(key): key for key in expected_keys}
    if len(expected_keys) != 4 or len(expected_by_normalized) != 4:
        failures.append("fixture must contain four distinct expected keys")

    marker_events = {
        marker: _one_marker(events, marker, failures) for marker in PHASE_MARKERS
    }
    root_event = _one_prefixed_marker(events, "idle198:root ", failures)
    final_event = _one_prefixed_marker(events, "idle198:final ", failures)
    event_positions = {id(event): index for index, event in enumerate(events)}
    marker_times = {
        marker: event_time(event)
        for marker, event in marker_events.items()
        if event is not None
    }
    if len(marker_times) == len(PHASE_MARKERS):
        ordered_times = [marker_times[marker] for marker in PHASE_MARKERS]
        ordered_positions = [
            event_positions[id(marker_events[marker])] for marker in PHASE_MARKERS
        ]
        if ordered_times != sorted(ordered_times) or ordered_positions != sorted(
            ordered_positions
        ):
            failures.append("scenario phase markers were not ordered in the perf log")

    root_fields = (
        parse_fields(str(root_event.get("message", "")), "idle198:root")
        if root_event
        else {}
    )
    final_fields = (
        parse_fields(str(final_event.get("message", "")), "idle198:final")
        if final_event
        else {}
    )
    if (
        root_event is not None
        and final_event is not None
        and len(marker_times) == len(PHASE_MARKERS)
        and not (
            event_positions[id(root_event)]
            < event_positions[id(marker_events["idle198:small:begin"])]
            and event_positions[id(marker_events["idle198:enlarged:end"])]
            < event_positions[id(final_event)]
            < event_positions[id(marker_events["idle198:done"])]
            and event_time(root_event) <= marker_times["idle198:small:begin"]
            and marker_times["idle198:enlarged:end"] <= event_time(final_event)
            and event_time(final_event) <= marker_times["idle198:done"]
        )
    ):
        failures.append("root/final markers did not bracket the ordered scenario phases")
    try:
        generation = int(root_fields.get("items_generation", ""))
    except ValueError:
        generation = -1
        failures.append("root marker did not contain a valid items_generation")
    required_root = {
        "is_fullscreen": "false",
        "items_len": "4",
        "pending_thumbs": "0",
        "target_mode": "targeted",
        "target_current": "true",
    }
    for name, expected in required_root.items():
        if root_fields.get(name) != expected:
            failures.append(
                f"root snapshot {name}={root_fields.get(name)!r}, expected {expected!r}"
            )
    required_final = {
        "is_fullscreen": "false",
        "items_len": "4",
        "pending_thumbs": "0",
        "target_mode": "targeted",
        "target_current": "true",
        "same_root": "true",
        "items_generation": str(generation),
    }
    for name, expected in required_final.items():
        if final_fields.get(name) != expected:
            failures.append(
                f"final snapshot {name}={final_fields.get(name)!r}, expected {expected!r}"
            )

    session_pids = {
        parsed
        for event in events
        if event.get("cat") == "session" and event.get("kind") == "start"
        for parsed in [_int_field(event, "pid")]
        if parsed is not None
    }
    if session_pids != {expected_pid}:
        environment_failures.append(
            f"perf session PID set was {sorted(session_pids)}, expected [{expected_pid}]"
        )

    samples = list(lifetime_samples)
    lifetime_proof_count = 0
    post_proof_sample_count = 0
    post_proof_invalid_count = 0
    if not samples:
        environment_failures.append("external lifetime/foreground samples are missing")
    else:
        def sample_matches_live_process(sample: dict[str, Any]) -> bool:
            return (
                _int_field(sample, "schema_version") == 2
                and _int_field(sample, "app_pid") == expected_pid
                and bool(sample.get("pre_alive"))
                and _int_field(sample, "pre_foreground_pid") == expected_pid
                and bool(sample.get("post_alive"))
                and _int_field(sample, "post_foreground_pid") == expected_pid
                and bool(sample.get("alive"))
                and _int_field(sample, "foreground_pid") == expected_pid
                and bool(sample.get("matches_expected_process"))
                and bool(str(sample.get("process_start_utc", "")).strip())
            )

        valid_final_indices = [
            index
            for index, sample in enumerate(samples)
            if "idle198:enlarged:end"
            in {str(value) for value in (sample.get("observed_markers") or [])}
            and sample_matches_live_process(sample)
        ]
        measurement_end_index = valid_final_indices[0] if valid_final_indices else None
        proof_samples = (
            samples[: measurement_end_index + 1]
            if measurement_end_index is not None
            else samples
        )
        lifetime_proof_count = len(proof_samples)
        post_proof_samples = (
            samples[measurement_end_index + 1 :]
            if measurement_end_index is not None
            else []
        )
        post_proof_sample_count = len(post_proof_samples)
        post_proof_invalid_count = sum(
            not sample_matches_live_process(sample) for sample in post_proof_samples
        )
        invalid_samples = [
            sample
            for sample in proof_samples
            if not sample_matches_live_process(sample)
        ]
        if invalid_samples:
            environment_failures.append(
                f"{len(invalid_samples)} proof samples did not match the live foreground process"
            )
        process_starts = {
            str(sample.get("process_start_utc", "")).strip() for sample in proof_samples
        }
        if len(process_starts) != 1 or not next(iter(process_starts), ""):
            environment_failures.append(
                "proof samples did not retain one non-empty process creation identity"
            )
        if measurement_end_index is None:
            environment_failures.append(
                "no live foreground proof sample observed idle198:enlarged:end"
            )
        sample_indices = [_int_field(sample, "sample_index") for sample in proof_samples]
        expected_indices = list(range(len(proof_samples)))
        if sample_indices != expected_indices:
            environment_failures.append(
                "external sample indices were missing, duplicated, or reordered"
            )
        elapsed_values = [_int_field(sample, "elapsed_ms") for sample in proof_samples]
        if any(value is None for value in elapsed_values):
            environment_failures.append("external samples contain an invalid elapsed_ms")
            elapsed_values = [value if value is not None else -1 for value in elapsed_values]
        gaps = [after - before for before, after in zip(elapsed_values, elapsed_values[1:])]
        if gaps and min(gaps) <= 0:
            environment_failures.append(
                "external sample elapsed times were duplicated or reversed"
            )
        if gaps and max(gaps) > 1000:
            environment_failures.append(
                f"external sample gap was {max(gaps)}ms, expected <=1000ms"
            )
        observed_sets = [
            {str(value) for value in (sample.get("observed_markers") or [])}
            for sample in proof_samples
        ]
        if observed_sets and "idle198:374:begin" in observed_sets[0]:
            environment_failures.append(
                "lifetime sampling started after idle198:374:begin was already observed"
            )
        first_observed_elapsed: dict[str, int] = {}
        previous_observed: set[str] = set()
        for elapsed, observed in zip(elapsed_values, observed_sets):
            if not previous_observed.issubset(observed):
                environment_failures.append(
                    "observed perf marker set moved backwards between lifetime samples"
                )
                break
            for marker in LIFETIME_MARKERS:
                if marker in observed and marker not in first_observed_elapsed:
                    first_observed_elapsed[marker] = elapsed
            previous_observed = observed
        missing_observations = [
            marker for marker in LIFETIME_MARKERS if marker not in first_observed_elapsed
        ]
        if missing_observations:
            environment_failures.append(
                f"lifetime samples never observed perf markers {missing_observations}"
            )
        elif [first_observed_elapsed[marker] for marker in LIFETIME_MARKERS] != sorted(
            first_observed_elapsed[marker] for marker in LIFETIME_MARKERS
        ):
            environment_failures.append(
                "perf marker observations were reordered in lifetime samples"
            )
        if (
            len(marker_times) == len(PHASE_MARKERS)
            and not missing_observations
            and elapsed_values
        ):
            quality_observation_index = next(
                index
                for index, observed in enumerate(observed_sets)
                if "idle198:374:begin" in observed
            )
            coverage_start_index = max(0, quality_observation_index - 1)
            measured_span_ms = (
                marker_times["idle198:enlarged:end"]
                - marker_times["idle198:374:begin"]
            ) * 1000.0
            external_span_ms = (
                first_observed_elapsed["idle198:enlarged:end"]
                - elapsed_values[coverage_start_index]
            )
            if external_span_ms < measured_span_ms:
                environment_failures.append(
                    "external foreground sampling did not span the measured perf phases"
                )

    if root_event is not None and final_event is not None:
        unexpected_nav = [
            event
            for event in events
            if event_time(root_event) < event_time(event) < event_time(final_event)
            and event.get("cat") == "nav"
            and event.get("kind") == "load_folder_begin"
        ]
        if unexpected_nav:
            failures.append("folder navigation changed during the convergence scenario")

    small_baseline: dict[str, Any] = {}
    if marker_events["idle198:374:begin"] is not None:
        quality_begin_t = marker_times["idle198:374:begin"]
        for key in expected_keys:
            candidates = [
                event
                for event in events
                if event.get("cat") == "thumb"
                and event.get("kind") == "load_phases"
                and _expected_event_key(event, expected_by_normalized) == key
                and event_time(event) < quality_begin_t
                and (_int_field(event, "evaluated_display_px") or 10**9) < 374
                and _positive_int(event, "display_output_width") is not None
                and _positive_int(event, "display_output_height") is not None
            ]
            if not candidates:
                failures.append(f"small baseline source load was missing for {key}")
                continue
            chosen = max(candidates, key=event_time)
            small_baseline[key] = {
                "t": event_time(chosen),
                "idx": chosen.get("idx"),
                "input_seq": chosen.get("input_seq"),
                "evaluated_display_px": chosen.get("evaluated_display_px"),
                "display_output_width": chosen.get("display_output_width"),
                "display_output_height": chosen.get("display_output_height"),
                "skip_cache": chosen.get("skip_cache"),
            }

    quiet_windows: dict[str, Any] = {}
    required_for_phases = all(marker_events[name] is not None for name in PHASE_MARKERS[:-1])
    if required_for_phases and generation >= 0:
        quiet_windows["quality374"] = _analyze_quality_phase(
            events,
            expected_keys,
            expected_by_normalized,
            generation,
            marker_times["idle198:small:begin"],
            marker_times["idle198:374:begin"],
            marker_times["idle198:374:end"],
            "quality374",
            failures,
        )
        quiet_windows["enlarged"] = _analyze_quality_phase(
            events,
            expected_keys,
            expected_by_normalized,
            generation,
            marker_times["idle198:374:end"],
            marker_times["idle198:enlarged:begin"],
            marker_times["idle198:enlarged:end"],
            "enlarged",
            failures,
        )
        quality_fixture = quiet_windows["quality374"]["fixture"]
        for key, baseline in small_baseline.items():
            quality = quality_fixture.get(key)
            if quality is not None and baseline.get("idx") != quality.get("idx"):
                failures.append(
                    f"small baseline {key}: idx {baseline.get('idx')} did not match "
                    f"quality374 idx {quality.get('idx')}"
                )
        observed_input_sequences = {
            _perf_input_seq(event)
            for event in events
            if event.get("cat") == "thumb"
            and _expected_event_key(event, expected_by_normalized) is not None
            and (
                (
                    event.get("kind") == "load_phases"
                    and event_time(event) < marker_times["idle198:374:begin"]
                    and (_int_field(event, "evaluated_display_px") or 10**9) < 374
                )
                or (
                    marker_times["idle198:small:begin"]
                    <= event_time(event)
                    < marker_times["idle198:374:end"]
                    and event.get("kind") in {"idle_upgrade_enqueue", "load_phases"}
                    and (
                        event.get("kind") != "load_phases"
                        or event.get("skip_cache") is True
                    )
                )
                or (
                    marker_times["idle198:374:end"]
                    <= event_time(event)
                    < marker_times["idle198:enlarged:end"]
                    and event.get("kind") in {"idle_upgrade_enqueue", "load_phases"}
                    and (
                        event.get("kind") != "load_phases"
                        or event.get("skip_cache") is True
                    )
                )
            )
        }
        if len(observed_input_sequences) != 1 or None in observed_input_sequences:
            failures.append(
                f"fixture work crossed input sequences {sorted(observed_input_sequences, key=str)}"
            )

    all_failures = [*environment_failures, *failures]

    return {
        "schema_version": 1,
        "status": "fail" if all_failures else "pass",
        "expected_pid": expected_pid,
        "expected_keys": expected_keys,
        "items_generation": generation,
        "markers": marker_times,
        "small_baseline": small_baseline,
        "quiet_windows": quiet_windows,
        "lifetime_samples": {
            "count": len(samples),
            "proof_count": lifetime_proof_count,
            "post_proof_count": post_proof_sample_count,
            "post_proof_invalid_count": post_proof_invalid_count,
            "first_elapsed_ms": samples[0].get("elapsed_ms") if samples else None,
            "last_elapsed_ms": samples[-1].get("elapsed_ms") if samples else None,
            "final_marker_samples": sum(
                sample.get("phase") == "final-marker-observed" for sample in samples
            ),
        },
        "environment_failures": environment_failures,
        "failures": all_failures,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("perf_log", type=Path)
    parser.add_argument("--fixture-dir", type=Path, required=True)
    parser.add_argument("--expected-pid", type=int, required=True)
    parser.add_argument("--lifetime-samples", type=Path, required=True)
    parser.add_argument("--json-out", type=Path, required=True)
    args = parser.parse_args()

    expected_files = sorted(args.fixture_dir.glob("idle198-*.png"))
    expected_keys = [str(path.resolve()) for path in expected_files]
    try:
        events = read_json_lines(args.perf_log)
        samples = read_json_lines(args.lifetime_samples)
    except (OSError, ValueError) as error:
        print(f"idle198-convergence: environment evidence error: {error}", file=sys.stderr)
        return 2
    report = analyze_idle198_convergence(events, expected_keys, args.expected_pid, samples)
    args.json_out.parent.mkdir(parents=True, exist_ok=True)
    args.json_out.write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + os.linesep,
        encoding="utf-8",
    )
    print(
        f"idle198-convergence: {report['status'].upper()} "
        f"failures={len(report['failures'])} samples={report['lifetime_samples']['count']}"
    )
    for failure in report["failures"]:
        print(f"  FAIL: {failure}")
    if report["status"] == "pass":
        return 0
    return 2 if report["environment_failures"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
