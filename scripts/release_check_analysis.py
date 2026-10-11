#!/usr/bin/env python3
"""Noninteractive release-check analysis. Missing witnesses never become PASS.

Canonical lifecycle reconciliation is Phase 3b; this module never starts an app.
All reports use schema_version=1 and exit precedence FAIL(1), SKIPPED(2), PASS(0).
"""
from __future__ import annotations

import argparse
import json
import math
from collections import Counter
from pathlib import Path

from analyze_perf import PerfEvents, analyze_idle_health, percentile, validate_delivery

REQUIRED_CASES = (
    "static-foreground", "static-background", "video-pin-background",
    "tray-close-while-loading", "tray-close-after-loading", "perf-cached", "perf-uncached",
)
HITCH_MS = 100.0
STARTUP_FIRST_FRAME_MAX_GAP_MS = 500.0
MIN_DRAWN_SAMPLES = 200
U64_MAX = (1 << 64) - 1
SOURCE_ORIGINS = {"source_generated", "source_intrinsic"}
CACHE_ORIGINS = {"upgradeable_cache", "final_cache", "drive_list_child_seed", "edit_preview_cache"}


def number(value: object) -> float | None:
    if isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value):
        return float(value)
    return None


def uint(value: object, *, positive: bool = False) -> bool:
    return type(value) is int and (1 if positive else 0) <= value <= U64_MAX


def request_identity(event: dict) -> tuple | None:
    """Only the registered canonical u64 ID identifies a request; seq never does."""
    key = event.get("key")
    if (not uint(event.get("request_id"), positive=True) or not isinstance(key, str) or not key
            or not uint(event.get("idx")) or not uint(event.get("items_gen"))):
        return None
    return event["request_id"], key, event["idx"], event["items_gen"]


def request_begins(events: list[dict]) -> dict[tuple, dict]:
    """Exclude missing/ambiguous registration rather than guessing from a result."""
    groups = {}
    for event in events:
        if event.get("cat") == "qa" and event.get("kind") == "request_begin":
            identity = request_identity(event)
            if identity is not None:
                groups.setdefault(identity[0], []).append(event)
    return {request_identity(group[0]): group[0] for group in groups.values()
            if len(group) == 1 and number(group[0].get("t")) is not None
            and group[0]["t"] >= 0 and uint(group[0].get("evaluated_display_px"), positive=True)}


def correlated_producer(event: dict, begins: dict[tuple, dict]) -> dict | None:
    identity = request_identity(event)
    begin = begins.get(identity)
    timestamp = number(event.get("t"))
    if begin is None or timestamp is None or timestamp < begin["t"]:
        return None
    if event.get("kind") == "source" and event.get("from_cache") is False and event.get("origin") in SOURCE_ORIGINS:
        return begin
    if event.get("kind") == "cache_read" and event.get("from_cache") is True and event.get("origin") in CACHE_ORIGINS:
        return begin
    return None


def delivery_evidence(result: dict, events: list[dict], expected_pid: int | None,
                      *, coverage_end_t: float | None = None) -> None:
    delivery = validate_delivery(events, expected_pid=expected_pid, coverage_end_t=coverage_end_t)
    result["evidence"]["delivery"] = delivery
    result["environment_errors"].extend(f"log delivery: {reason}" for reason in delivery["reasons"])


def report(case_id: str) -> dict:
    return {"schema_version": 1, "case_id": case_id, "failures": [],
            "environment_errors": [], "warnings": [], "metrics": {}, "evidence": {}}


def finish(result: dict) -> dict:
    result["verdict"] = ("FAIL" if result["failures"] else "SKIPPED" if result["environment_errors"]
                         else "PASS_WITH_WARNINGS" if result["warnings"] else "PASS")
    result["exit_code"] = {"FAIL": 1, "SKIPPED": 2}.get(result["verdict"], 0)
    return result


def read_json(path: Path) -> dict:
    value = json.loads(path.read_text(encoding="utf-8-sig"),
                       parse_constant=lambda s: (_ for _ in ()).throw(ValueError(s)))
    if not isinstance(value, dict):
        raise ValueError("JSON root must be an object")
    return value


def load_events_strict(path: Path) -> tuple[list[dict], list[str]]:
    """Unlike the exploratory CLI, the release gate retains malformed-line errors."""
    events, errors = PerfEvents(), []
    try:
        with path.open(encoding="utf-8-sig") as stream:
            for line_no, line in enumerate(stream, 1):
                if not line.strip():
                    continue
                if not line.endswith("\n"):
                    errors.append(f"incomplete JSONL tail at line {line_no}")
                try:
                    event = json.loads(line, parse_constant=lambda s: (_ for _ in ()).throw(ValueError(s)))
                    if (not isinstance(event, dict) or number(event.get("t")) is None
                            or not isinstance(event.get("cat"), str) or not isinstance(event.get("kind"), str)):
                        raise ValueError("event needs finite t and string cat/kind")
                    event["_line"] = line_no
                    events.append(event)
                except (ValueError, TypeError) as exc:
                    errors.append(f"malformed log line {line_no}: {exc}")
    except (OSError, UnicodeError) as exc:
        errors.append(f"cannot read perf log: {exc}")
    if not events:
        errors.append("perf log contains no valid events")
    events.evidence_errors.extend(errors)
    return events, errors


def session_witness(events: list[dict], expected_pid: int | None, errors: list[str]) -> dict:
    sessions = [e for e in events if (e.get("cat"), e.get("kind")) == ("session", "start")]
    if len(sessions) != 1:
        errors.append("exactly one session.start is required")
        return {}
    session = sessions[0]
    pid = session.get("pid")
    if not isinstance(pid, int) or isinstance(pid, bool) or pid <= 0:
        errors.append("invalid session PID")
    if expected_pid is None or pid != expected_pid:
        errors.append("session PID does not match the exact runner PID")
    if any(e.get("cat") == "log" and e.get("kind") == "diagnostic_dropped" for e in events):
        errors.append("diagnostic_dropped: evidence is incomplete")
    return session


def convert_clock_window(events: list[dict], sampler: dict, expected_pid: int | None = None) -> dict:
    result = report(sampler.get("case_id", "clock-window"))
    errors = result["environment_errors"]
    expected_pid = expected_pid if expected_pid is not None else sampler.get("expected_pid")
    session = session_witness(events, expected_pid, errors)
    anchor_ms, anchor_t = number(session.get("wall_unix_ms")), number(session.get("wall_t"))
    fields = ("measure_start_unix_ms", "measure_end_unix_ms", "sleep_start_unix_ms", "sleep_end_unix_ms")
    boundaries = {field: number(sampler.get(field)) for field in fields}
    wall = number(sampler.get("wall_seconds"))
    if anchor_ms is None or anchor_ms <= 0 or anchor_t is None or anchor_t < 0:
        errors.append("missing or invalid session clock anchor; no Process.StartTime fallback")
    if any(v is None or v <= 0 for v in boundaries.values()) or wall is None or wall <= 0:
        errors.append("missing or invalid UTC/sleep/Stopwatch boundaries")
    if errors:
        return finish(result)
    start, end, sleep_start, sleep_end = [boundaries[f] for f in fields]
    if end <= start or sleep_end <= sleep_start:
        errors.append("reversed measurement or sleep boundary")
    if abs((end - start) / 1000 - wall) > 0.1:
        errors.append("wall clock changed or UTC/Stopwatch intervals disagree (>0.1s)")
    times = [anchor_t + (value - anchor_ms) / 1000 for value in (start, end, sleep_start, sleep_end)]
    included = sleep_start <= start < end <= sleep_end
    if not included or times[0] < 0:
        errors.append("measurement is not wholly inside the worker sleep interval")
    result["evidence"]["clock"] = {
        "anchor": {"pid": session.get("pid"), "wall_unix_ms": anchor_ms, "wall_t": anchor_t},
        **boundaries, "wall_seconds": wall, "start_t": times[0], "end_t": times[1],
        "sleep_start_t": times[2], "sleep_end_t": times[3], "included": included,
    }
    return finish(result)


def interval(event: dict, duration_field: str = "total_ms") -> tuple[float, float] | None:
    end = number(event.get("end_t"))
    if end is None:
        end = number(event.get("t"))
    start = number(event.get("start_t"))
    if start is None:
        ms = number(event.get(duration_field))
        if ms is None or ms < 0 or end is None:
            return None
        start = end - ms / 1000
    return (start, end) if end is not None and end >= start else None


def stats(values: list[float]) -> dict:
    return {"n": len(values), "p50_ms": percentile(values, .5), "p95_ms": percentile(values, .95),
            "max_ms": max(values) if values else None}


def normalize_search_path(value: str) -> str:
    """Compare Windows physical addresses; thumbnail owner keys remain exact."""
    value = value.replace("\\", "/")
    if value.lower().startswith("//?/unc/"):
        value = "//" + value[8:]
    elif value.startswith("//?/"):
        value = value[4:]
    return value.rstrip("/").casefold()


def operation_errors(evidence: dict, expected_pid: int | None) -> list[str]:
    errors = []
    for field in ("folder_open", "search", "exit", "first_ai"):
        if field in evidence and not isinstance(evidence[field], dict):
            return [f"invalid operation evidence object: {field}"]
    if evidence.get("pid") != expected_pid:
        errors.append("case evidence PID mismatch or missing")
    if evidence.get("folder_open", {}).get("completed") is not True:
        errors.append("folder-open completion witness missing")
    navigation = evidence.get("navigation", [])
    if not isinstance(navigation, list) or len(navigation) != 5:
        errors.append("exactly five independent Ctrl+Down receipts are required")
    else:
        for i, nav in enumerate(navigation, 1):
            if (not isinstance(nav, dict) or nav.get("receipt") is not True
                    or not isinstance(nav.get("before_generation"), int) or isinstance(nav.get("before_generation"), bool)
                    or not isinstance(nav.get("after_generation"), int) or isinstance(nav.get("after_generation"), bool)
                    or nav["after_generation"] <= nav["before_generation"]
                    or not isinstance(nav.get("before_path"), str) or not nav["before_path"]
                    or not isinstance(nav.get("after_path"), str) or not nav["after_path"]
                    or normalize_search_path(nav["before_path"]) == normalize_search_path(nav["after_path"])):
                errors.append(f"Ctrl+Down {i} lacks receipt/generation/location change")
    search = evidence.get("search", {})
    if not all(search.get(f) is True for f in ("ctrl_g_receipt", "text_receipt", "completed")):
        errors.append("Ctrl+G/text/search completion witness missing")
    expected, actual = search.get("expected_hits"), search.get("actual_hits")
    if (not isinstance(expected, list) or not expected or not isinstance(actual, list)
            or not all(isinstance(p, str) for p in expected + actual)
            or {normalize_search_path(p) for p in expected} != {normalize_search_path(p) for p in actual}):
        errors.append("search hit set does not match the fixture")
    exit_evidence = evidence.get("exit", {})
    if exit_evidence.get("process_exited") is not True or exit_evidence.get("forced") is not False:
        errors.append("normal process termination witness missing")
    return errors


def target_keys(value: object) -> list[str]:
    if not isinstance(value, list):
        return []
    return [v if isinstance(v, str) else v["key"] for v in value
            if isinstance(v, str) and v or isinstance(v, dict) and isinstance(v.get("key"), str) and v["key"]]


def cache_errors(events: list[dict], evidence: dict, case_id: str) -> list[str]:
    """Full-key read provenance for the intended subtree, never filename matches."""
    if case_id not in ("static-foreground", "static-background", "video-pin-background", "perf-cached", "perf-uncached"):
        return []
    cache = evidence.get("cache", {})
    if not isinstance(cache, dict):
        return ["invalid case cache provenance object"]
    mode = "uncached" if case_id == "perf-uncached" else "cached"
    opens = cache.get("opens", [])
    if cache.get("mode") != mode or not isinstance(opens, list) or not opens:
        return [f"{mode} case cache/open provenance missing"]
    if case_id.startswith("perf-"):
        manifest = evidence.get("fixture_manifest", {})
        recipe = manifest.get("perf", {}).get(mode, {})
        order, many = recipe.get("navigation_order"), recipe.get("many_folders")
        if (not isinstance(many, str) or not many or not isinstance(order, list) or len(order) != 6
                or not all(isinstance(p, str) and p for p in order)):
            return ["verified perf fixture manifest many-folder/navigation recipe missing"]
        loads = recipe.get("load_order")
        if (not isinstance(loads, list) or len(loads) != 256
                or not all(isinstance(p, str) and p for p in loads)
                or len({normalize_search_path(p) for p in loads}) != 256):
            return ["verified perf fixture manifest distinct-folder load recipe missing"]
        intended = [many, *loads, *order]
        for i, nav in enumerate(evidence.get("navigation", [])):
            if (i >= 5 or not isinstance(nav, dict)
                    or normalize_search_path(nav.get("before_path", "")) != normalize_search_path(order[i])
                    or normalize_search_path(nav.get("after_path", "")) != normalize_search_path(order[i + 1])):
                return ["Ctrl+Down observed route differs from the fixture navigation order"]
    else:
        intended = [opens[0].get("folder_path", "")] if isinstance(opens[0], dict) else []
    valid_folders = set()
    begins = request_begins(events)
    for opened in opens:
        if not isinstance(opened, dict) or not opened.get("folder_path"):
            continue
        keys = target_keys(opened.get("target_keys", []))
        generation = opened.get("items_gen")
        if not keys or not isinstance(generation, int) or isinstance(generation, bool):
            continue
        folder = normalize_search_path(opened["folder_path"])
        normalized_order = [normalize_search_path(p) for p in order] if case_id.startswith("perf-") else []
        if folder in normalized_order:
            position = normalized_order.index(folder)
            navs = evidence.get("navigation", [])
            nav_index = max(0, position - 1)
            generation_field = "before_generation" if position == 0 else "after_generation"
            if len(navs) <= nav_index or navs[nav_index].get(generation_field) != generation:
                continue
        if case_id.startswith("perf-") and folder in [normalize_search_path(p) for p in recipe.get("load_order", [])]:
            loads = evidence.get("folder_loads", [])
            if not any(isinstance(load, dict) and normalize_search_path(load.get("after_path", "")) == folder
                       and load.get("after_generation") == generation for load in loads):
                continue
        if case_id == "video-pin-background" and evidence.get("target_key") not in keys:
            continue
        if mode == "cached":
            required_keys = [evidence["target_key"]] if case_id == "video-pin-background" else keys
            first_reads = []
            for key in required_keys:
                producers = [e for e in events if e.get("cat") == "qa" and e.get("kind") in ("source", "cache_read")
                             and e.get("key") == key and e.get("items_gen") == generation]
                if any(correlated_producer(e, begins) is None for e in producers):
                    first_reads.append(False)
                    continue
                producers.sort(key=lambda e: (begins[request_identity(e)]["t"], e["t"]))
                first_reads.append(bool(producers and producers[0].get("kind") == "cache_read"
                                        and producers[0].get("from_cache") is True))
            if all(first_reads):
                valid_folders.add(folder)
        elif opened.get("pre_open_absent") is True:
            first_sources = []
            for key in keys:
                producers = [e for e in events if e.get("cat") == "qa" and e.get("kind") in ("source", "cache_read")
                             and e.get("key") == key and e.get("items_gen") == generation]
                # Every candidate needs its actual begin, including the first cache hit.
                # Missing provenance cannot silently sort behind a later source result.
                if any(correlated_producer(e, begins) is None for e in producers):
                    first_sources.append(False)
                    continue
                producers.sort(key=lambda e: (begins[request_identity(e)]["t"], e["t"]))
                first_sources.append(bool(producers and producers[0].get("kind") == "source"))
            if all(first_sources):
                valid_folders.add(folder)
    missing = [p for p in intended if normalize_search_path(p) not in valid_folders]
    return [] if intended and not missing else [f"{mode} intended-folder cache/source provenance not proven: {missing}"]


def chord_errors(events: list[dict], evidence: dict, *, navigation: bool = True) -> list[str]:
    """Actual synthetic delivery plus observable consumer outcomes, not queue ACKs."""
    errors = []
    hwnd = evidence.get("observation", {}).get("root_hwnd")
    if not isinstance(hwnd, int) or isinstance(hwnd, bool) or hwnd <= 0:
        return ["exact root HWND for synthetic input correlation missing"]

    def presses(key: str, start: float, end: float) -> list[dict]:
        return [e for e in events if e.get("cat") == "qa" and e.get("kind") == "key_materialized"
                and e.get("key") == key and e.get("ctrl") is True
                and e.get("shift") is False and e.get("alt") is False
                and e.get("pressed") is True and e.get("repeat") is False
                and e.get("target_viewport") == "ROOT" and e.get("target_hwnd") == hwnd
                and number(e.get("t")) is not None and start - .001 <= e["t"] <= end + .001]

    def bounds(value: dict) -> tuple[float, float] | None:
        start, end = number(value.get("start_t")), number(value.get("end_t"))
        return (start, end) if start is not None and end is not None and end > start else None

    prior_end = None
    if navigation:
        windows = evidence.get("navigation", [])
        if not isinstance(windows, list) or len(windows) != 5:
            errors.append("five Ctrl+Down correlation windows missing")
        else:
            for i, window in enumerate(windows, 1):
                span = bounds(window) if isinstance(window, dict) else None
                if span is None or (prior_end is not None and span[0] < prior_end):
                    errors.append(f"Ctrl+Down {i} missing/overlapping observation interval")
                elif len(presses("Down", *span)) != 1:
                    errors.append(f"Ctrl+Down {i} lacks exactly one ROOT materialized nonrepeat chord")
                if span:
                    prior_end = span[1]
    search = evidence.get("search", {})
    span = bounds(search) if isinstance(search, dict) else None
    query = search.get("expected_query") if isinstance(search, dict) else None
    if span is None or not isinstance(query, str) or not query:
        errors.append("Ctrl+G/search fixed-query correlation interval missing")
    elif search.get("actual_query", evidence.get("observation", {}).get("search_query")) != query:
        errors.append("normal search TextEdit did not preserve the fixed query")
    elif len(presses("G", *span)) != 1:
        errors.append("Ctrl+G lacks exactly one ROOT materialized nonrepeat chord")
    elif not any(e.get("cat") == "qa" and e.get("kind") == "text" and e.get("text") == query
                 and e.get("root_hwnd") == hwnd and span[0] - .001 <= e.get("t", -1) <= span[1] + .001
                 for e in events):
        errors.append("fixed search text was not delivered to the same ROOT interval")
    return errors


def static_request_errors(events: list[dict], evidence: dict, case_id: str) -> list[str]:
    if case_id not in ("static-foreground", "static-background"):
        return []
    px = number(evidence.get("requested_px"))
    if px is None or not 366 <= px <= 382:
        return ["static actual requested pixels outside 374±8 regression range [366,382]"]
    opens = evidence.get("cache", {}).get("opens", [])
    if not opens or not isinstance(opens[0], dict):
        return ["static request target/generation proof missing"]
    keys, gen = target_keys(opens[0].get("target_keys")), opens[0].get("items_gen")
    begins = request_begins(events)
    if not any(e.get("cat") == "qa" and e.get("kind") in ("source", "cache_read") and e.get("key") in keys
               and e.get("items_gen") == gen and (begin := correlated_producer(e, begins)) is not None
               and number(begin.get("evaluated_display_px")) == px for e in events):
        return ["static worker request has no matching actual display pixels/key/generation witness"]
    return []


def folder_load_errors(evidence: dict, case_id: str) -> list[str]:
    """A fixed fresh-folder workload, with ordinary opens and observed outcomes."""
    mode = "uncached" if case_id == "perf-uncached" else "cached"
    recipe = evidence.get("fixture_manifest", {}).get("perf", {}).get(mode, {})
    order, loads = recipe.get("load_order"), evidence.get("folder_loads")
    if (not isinstance(order, list) or len(order) != 256
            or not all(isinstance(p, str) and p for p in order)
            or len({normalize_search_path(p) for p in order}) != 256
            or not isinstance(loads, list) or len(loads) != 256):
        return ["256 distinct fixture folder-load operations required"]
    previous_path, previous_generation, previous_end = recipe.get("many_folders"), None, None
    errors = []
    for i, (path, load) in enumerate(zip(order, loads), 1):
        span = interval(load) if isinstance(load, dict) else None
        if (span is None or span[0] >= span[1] or previous_end is not None and span[0] < previous_end
                or any(not isinstance(load.get(f), int) or isinstance(load[f], bool)
                       for f in ("before_generation", "after_generation"))
                or load["after_generation"] <= load["before_generation"]
                or previous_generation is not None and load["before_generation"] != previous_generation
                or not isinstance(load.get("before_path"), str) or not isinstance(load.get("after_path"), str)
                or normalize_search_path(load["before_path"]) != normalize_search_path(previous_path or "")
                or normalize_search_path(load["after_path"]) != normalize_search_path(path)):
            errors.append(f"folder-load {i} invalid fixture route/generation/interval")
            continue
        previous_path, previous_generation, previous_end = path, load["after_generation"], span[1]
    navs = evidence.get("navigation", [])
    nav_start = number(navs[0].get("start_t")) if isinstance(navs, list) and navs and isinstance(navs[0], dict) else None
    if previous_end is None or nav_start is None or previous_end > nav_start:
        errors.append("folder-load workload must complete before the five Ctrl+Down operations")
    return errors


def analyze_perf_smoke(events: list[dict], *, expected_pid: int | None = None,
                       case_id: str = "perf-smoke", evidence: dict | None = None,
                       load_errors: list[str] | None = None) -> dict:
    result = report(case_id)
    errors = result["environment_errors"]
    errors.extend(load_errors or [])
    valid_events = []
    for event in events:
        if not isinstance(event, dict) or number(event.get("t")) is None:
            errors.append("malformed event timestamp/object")
        elif event.get("cat") in ("frame", "ui") and event.get("kind") in (
                "begin", "tail_repaint", "update_breakdown", "pre_grid_breakdown") and (
                not isinstance(event.get("n"), int) or isinstance(event.get("n"), bool)):
            errors.append("malformed frame identity")
        else:
            valid_events.append(event)
    events = valid_events
    session_witness(events, expected_pid, errors)
    delivery_evidence(result, events, expected_pid,
                      coverage_end_t=max((e["t"] for e in events if e.get("cat") != "log"), default=0))
    evidence = evidence or {}
    if not isinstance(evidence, dict):
        errors.append("case evidence must be an object")
        evidence = {}
    if case_id == "first-ai":
        if evidence.get("pid") != expected_pid:
            errors.append("case evidence PID mismatch or missing")
        exit_evidence = evidence.get("exit", {})
        if exit_evidence.get("process_exited") is not True or exit_evidence.get("forced") is not False:
            errors.append("normal process termination witness missing")
    else:
        errors.extend(operation_errors(evidence, expected_pid))
    errors.extend(cache_errors(events, evidence, case_id))
    if case_id != "first-ai":
        errors.extend(chord_errors(events, evidence))
    if case_id in ("perf-cached", "perf-uncached"):
        errors.extend(folder_load_errors(evidence, case_id))
    if not any(e.get("cat") == "qa" and e.get("kind") == "on_exit" and e.get("case_id") == case_id
               and e.get("flush_complete") is True for e in events):
        errors.append("normal on_exit/flush diagnostic witness missing")
    result["evidence"]["operations"] = evidence
    frames = sorted([e for e in events if (e.get("cat"), e.get("kind")) == ("frame", "begin")
                     and number(e.get("t")) is not None], key=lambda e: e["t"])
    frame_numbers = [e.get("n") for e in frames]
    if (len(frames) < 2 or not frame_numbers or frame_numbers[0] != 1
            or any(not isinstance(n, int) or isinstance(n, bool) for n in frame_numbers)
            or len(set(frame_numbers)) != len(frame_numbers)
            or any(b != a + 1 for a, b in zip(frame_numbers, frame_numbers[1:]))):
        errors.append("missing first frame n=1 or duplicate/nonconsecutive frame.begin identity")
    tails, inner, pre_grid = {}, {}, []
    pre_grid_numbers, duplicate_pre_grid_numbers = set(), set()
    for event in events:
        if event.get("cat") != "ui":
            continue
        kind = event.get("kind")
        if kind in ("tail_repaint", "update_breakdown"):
            target = tails if kind == "tail_repaint" else inner
            n = event.get("n")
            if n in target or n not in frame_numbers:
                errors.append(f"unmatched/duplicate {kind} frame {n}")
            target[n] = event
        elif kind == "pre_grid_breakdown":
            ms = number(event.get("total_ms"))
            if ms is None or ms < 0 or event.get("n") not in frame_numbers:
                errors.append("invalid pre_grid sample/frame")
            else:
                n = event["n"]
                if n in pre_grid_numbers and n not in duplicate_pre_grid_numbers:
                    errors.append(f"duplicate pre_grid sample for frame {n}")
                    duplicate_pre_grid_numbers.add(n)
                pre_grid_numbers.add(n)
                pre_grid.append(event)
    updates = {}
    drawn, gaps = [], []
    for previous, current in zip(frames, frames[1:]):
        n, gap_ms = previous.get("n"), (current["t"] - previous["t"]) * 1000
        tail = tails.get(n)
        if tail is None or not isinstance(tail.get("action"), str):
            errors.append(f"missing tail for frame {n}")
        if tail and tail.get("action") == "request_repaint":
            drawn.append(gap_ms)
        ms = number(current.get("prev_update_ms"))
        span = interval({"start_t": current.get("prev_update_start_t"),
                         "end_t": current.get("prev_update_end_t")})
        if current.get("n") != n + 1:
            errors.append(f"outer previous frame identity missing before frame {current.get('n')}")
        elif ms is None or ms < 0 or span is None:
            errors.append(f"missing outer update measurement/endpoints for frame {n}")
        else:
            updates[n] = {"n": n, "start_t": span[0], "end_t": span[1], "total_ms": ms,
                          "outer_measured": True, "inner": inner.get(n, {})}
        if gap_ms > HITCH_MS + 1e-9:
            gaps.append({"n": n, "next_n": current.get("n"), "start_t": previous["t"],
                         "end_t": current["t"], "gap_ms": gap_ms})
    for n, event in inner.items():
        ms, span = number(event.get("total_ms")), interval(event)
        if ms is None or ms < 0 or span is None:
            errors.append(f"invalid inner update for frame {n}")
        elif n not in updates:
            updates[n] = {"n": n, "start_t": span[0], "end_t": span[1], "total_ms": ms,
                          "outer_measured": False, "inner": event}
        elif ms > updates[n]["total_ms"]:
            # Inner is a partial measure but still exposes a real >100ms update.
            updates[n]["total_ms"] = ms
    startup_begins = sorted([e for e in events if (e.get("cat"), e.get("kind")) == ("nav", "load_folder_begin")],
                            key=lambda e: e.get("t", 0))
    startup = startup_begins[0] if startup_begins else None
    startup_ends = [e for e in events if (e.get("cat"), e.get("kind")) == ("nav", "load_folder_end")
                    and startup and e.get("seq", 0) == startup.get("seq", 0) and e.get("t", -1) >= startup["t"]]
    startup_end = min((e["t"] for e in startup_ends), default=None)
    ai = evidence.get("first_ai", {})
    if not isinstance(ai, dict):
        ai = {}
    ai_ready = (ai.get("included") is True and ai.get("first") is True and ai.get("started") is True
                and number(ai.get("start_t")) is not None and number(ai.get("end_t")) is not None
                and ai["end_t"] > ai["start_t"] and all(ai.get(f) for f in ("model", "backend", "pack")))
    if ai.get("included") is True and not ai_ready:
        errors.append("initial AI operation/backend/model/pack witness incomplete")
    if ai.get("included") is True or case_id == "first-ai":
        ai_rows = [e for e in events if e.get("cat") == "qa" and e.get("kind") == "first_ai"
                   and e.get("case_id") == case_id]
        if not ai_ready or len(ai_rows) != 1 or not all(
                ai_rows[0].get(field) == ai.get(field) for field in
                ("included", "first", "started", "start_t", "end_t", "model", "backend", "pack")):
            errors.append("explicit optional first AI operation diagnostic witness missing/mismatched")
            ai_ready = False
    result["evidence"]["ai"] = ai if ai else {"included": False, "status": "AI未実施"}

    def classify_update(row: dict) -> dict:
        n, total = row["n"], row["total_ms"]
        detail = row["inner"]
        if not detail:
            errors.append(f"slow update {n} lacks stage breakdown")
        stages = {f: float(v) for f, v in detail.items() if f.endswith("_ms") and f != "total_ms"
                  and not f.startswith(("fs_", "still_image_display_indices", "get_nav_indices"))
                  and number(v) is not None}
        viewport = stages.get("fullscreen_viewport_ms", 0)
        scans = [e for e in events if (e.get("cat"), e.get("kind")) == ("nav", "lf_scan")
                 and startup and startup_end is not None and e.get("seq", 0) == startup.get("seq", 0)
                 and interval(e, "ms") is not None
                 and (e.get("n") == n or (e.get("n") is None and
                     row["start_t"] - .001 <= interval(e, "ms")[0]
                     and interval(e, "ms")[1] <= row["end_t"] + .001))
                 and startup["t"] - .001 <= interval(e, "ms")[0]
                 and interval(e, "ms")[1] <= startup_end + .001]
        scan = max((number(e.get("ms")) or 0 for e in scans), default=0)
        category, dominant = "unexplained", None
        if not stages:
            category = "incomplete-evidence"
        elif scan > HITCH_MS and scan > total / 2:
            category, dominant = "known-startup-scan", "lf_scan"
        elif (ai_ready and row["start_t"] < ai["end_t"] and row["end_t"] > ai["start_t"]
              and (tails.get(n, {}).get("action") == "request_repaint_after_ai_upscale" or
                   (tails.get(n, {}).get("action") == "request_repaint_after_observed"
                    and tails[n].get("early_return") is True
                    and (number(tails[n].get("delay_ms")) or 0) > 0))
              and viewport > HITCH_MS and viewport > total / 2):
            category, dominant = "known-first-ai", "fullscreen_viewport_ms"
        residual = []
        for stage, duration in stages.items():
            if category == "known-first-ai" and stage == dominant:
                continue
            # lf_scan is nested in the startup stage; preserve the remaining cost.
            if category == "known-startup-scan" and stage == "startup_and_setup_ms":
                duration = max(0, duration - scan)
            if duration > HITCH_MS:
                residual.append({"stage": stage, "ms": duration})
        inner_ms = number(detail.get("total_ms"))
        if row["outer_measured"] and inner_ms is not None and total - inner_ms > HITCH_MS:
            residual.append({"stage": "outside_inner_update_frame", "ms": total - inner_ms})
        categories = [category]
        if category.startswith("known-") and residual:
            categories.append("unexplained")
        return {**{k: v for k, v in row.items() if k != "inner"}, "categories": categories,
                "dominant_stage": dominant, "lf_scan_ms": scan or None, "viewport_ms": viewport,
                "inner_total_ms": inner_ms, "other_slow_stages": residual, "stages_ms": stages}

    slow_updates = [classify_update(row) for row in sorted(updates.values(), key=lambda r: (r["start_t"], r["n"]))
                    if row["total_ms"] > HITCH_MS]
    slow_by_n = {row["n"]: row for row in slow_updates}
    # Use the earliest operation interval, not a small startup timestamp. The
    # initial command-line folder load is startup; the explicit workload starts
    # with folder_loads (or first_ai in the optional independent session).
    operation_windows = [evidence.get("search"), evidence.get("first_ai"), evidence.get("folder_open")]
    for field in ("folder_loads", "navigation"):
        windows = evidence.get(field)
        if isinstance(windows, list):
            operation_windows.extend(windows)
    operation_starts = [span[0] for window in operation_windows if isinstance(window, dict)
                        and (span := interval(window)) is not None]
    first_operation_t = min(operation_starts, default=None)
    startup_first_frames = [e for e in events if (e.get("cat"), e.get("kind")) == ("startup", "first_frame")]
    init_completions = [e for e in events if (e.get("cat"), e.get("kind")) == ("startup", "indexer_manager_new")]

    def startup_first_frame_gap(gap: dict, tail: dict) -> bool:
        update = updates[gap["n"]]
        # early_return alone also describes fullscreen/navigation exits. Prove
        # that the startup init worker was still pending after this update, so
        # the earlier startup-overlay guard owned the return. Perf event t is
        # rounded to milliseconds; require a full 1ms margin for that witness.
        return (gap["n"] == 1 and gap["next_n"] == 2
                and gap["gap_ms"] <= STARTUP_FIRST_FRAME_MAX_GAP_MS + 1e-9
                and tail.get("early_return") is True and tail.get("action") == "request_repaint"
                and number(tail.get("delay_ms")) == 0 and 1 not in pre_grid_numbers
                and update["outer_measured"] and update["total_ms"] <= HITCH_MS
                and gap["start_t"] - .001 <= tail["t"] <= update["end_t"] + .001
                and first_operation_t is not None and gap["end_t"] < first_operation_t
                and len(startup_first_frames) == 1
                and abs(startup_first_frames[0]["t"] - gap["start_t"]) <= .001
                and len(init_completions) == 1
                and (init_span := interval(init_completions[0], "ms")) is not None
                and update["start_t"] - .001 <= init_span[0] <= update["end_t"]
                and init_completions[0]["t"] > update["end_t"] + .001)

    for gap in gaps:
        n, tail = gap["n"], tails.get(gap["n"], {})
        if n not in updates or not tail:
            gap["categories"] = ["incomplete-evidence"]
        elif n in slow_by_n:
            gap["categories"] = list(slow_by_n[n]["categories"])
            outside_ms = (gap["end_t"] - updates[n]["end_t"]) * 1000
            if (any(c.startswith("known-") for c in gap["categories"]) and tail.get("action") != "none"
                    and outside_ms > HITCH_MS and "unexplained" not in gap["categories"]):
                gap["categories"].append("unexplained")
        elif startup_first_frame_gap(gap, tail):
            gap["categories"] = ["known-startup-first-frame"]
            gap["first_operation_t"] = first_operation_t
            gap["upper_bound_ms"] = STARTUP_FIRST_FRAME_MAX_GAP_MS
        elif tail.get("action") == "none":
            gap["categories"] = ["normal-sleep"]
        elif tail.get("action") == "request_repaint_after_idle_upgrade":
            delay = number(tail.get("idle_upgrade_delay_ms"))
            # Scheduled wake begins after update ends, not frame.begin.
            wake = (gap["end_t"] - updates[n]["end_t"]) * 1000
            if delay is not None and delay > 0 and abs(wake - delay) <= 20:
                gap["categories"] = ["normal-idle-upgrade-wake"]
            else:
                gap["categories"] = ["unexplained"]
        else:
            gap["categories"] = ["unexplained"]
    unexplained_frames = sorted({r["n"] for r in gaps + slow_updates if "unexplained" in r["categories"]})
    if unexplained_frames:
        result["failures"].append(f"unexplained >100ms gap/update in frames {unexplained_frames}")
    if any(any(c.startswith("known-") for c in r["categories"]) for r in slow_updates):
        result["warnings"].append("known startup/initial AI stalls retained; not a shipping waiver")
    if any("known-startup-first-frame" in g["categories"] for g in gaps):
        result["warnings"].append("known-startup-first-frame gap retained (user accepted 2026-10-11; maximum 500ms)")
    drawn_stats = stats(drawn)
    below = sum(ms < 16 - 1e-9 for ms in drawn)
    ratio = below / len(drawn) if drawn else None
    minimum = None if case_id == "first-ai" else MIN_DRAWN_SAMPLES
    drawn_stats.update({"below_16ms": below, "below_16ms_ratio": ratio,
                        "denominator": "previous tail exactly request_repaint", "minimum_samples": minimum})
    if minimum is not None and len(drawn) < minimum:
        errors.append(f"drawn sample count {len(drawn)} < provisional minimum {MIN_DRAWN_SAMPLES}")
    if ratio is not None and ratio < .97:
        result["warnings"].append("drawn <16ms ratio below 97% target (WARN only)")
    pre_grid.sort(key=lambda e: (e["n"], e["t"]))
    pg_stats = stats([e["total_ms"] for e in pre_grid])
    if not pre_grid:
        errors.append("pre_grid total samples missing")
    elif pg_stats["p95_ms"] > .3:
        result["failures"].append("raw pre_grid.total_ms p95 exceeds 0.3ms")
    pg_stats["slow_frames"] = [{"n": e["n"], "total_ms": e["total_ms"],
                                "stages_ms": {k: v for k, v in e.items() if k.endswith("_ms") and k != "total_ms"}}
                               for e in pre_grid if e["total_ms"] > .3]
    classification_stats = {}
    for category in ("normal-sleep", "normal-idle-upgrade-wake", "known-startup-first-frame",
                     "known-startup-scan", "known-first-ai", "unexplained"):
        matching_gaps = [g for g in gaps if category in g["categories"]]
        matching_updates = [u for u in slow_updates if category in u["categories"]]
        classification_stats[category] = {
            "frames": len({r["n"] for r in matching_gaps + matching_updates}),
            "gaps": len(matching_gaps), "updates": len(matching_updates),
            "max_gap_ms": max((g["gap_ms"] for g in matching_gaps), default=None),
            "max_update_ms": max((u["total_ms"] for u in matching_updates), default=None),
            "max_lf_scan_ms": max((u["lf_scan_ms"] or 0 for u in matching_updates), default=None),
            "max_viewport_ms": max((u["viewport_ms"] for u in matching_updates), default=None),
        }
    result["metrics"] = {"total_frames": len(frames), "drawn": drawn_stats, "pre_grid": pg_stats,
                         "gap_count": len(gaps), "slow_update_count": len(slow_updates),
                         "unexplained_count": len(unexplained_frames),
                         "gap_categories": dict(Counter(c for r in gaps for c in r["categories"])),
                         "update_categories": dict(Counter(c for r in slow_updates for c in r["categories"])),
                         "classifications": classification_stats}
    result["evidence"].update({"gaps": gaps, "slow_updates": slow_updates})
    return finish(result)


def analyze_case_evidence(events: list[dict], evidence: dict, expected_pid: int | None = None,
                          *, setup_deadline_t: float | None = None) -> dict:
    """Validate actual diagnostic witnesses; recipe claims alone prove no app work.

    qa.request_begin/source/cache_read/close_accept/tray_hide/finalized are
    read-only projections of the canonical lifecycle; thumb timing is not proof.
    A finalized event's request_id must match source work, independently of seq.
    Tray reproduction must be complete by the converted measurement start.
    Missing lifecycle or log-delivery witnesses leave the run SKIPPED.
    """
    case_id = evidence.get("case_id", "evidence")
    result = report(case_id)
    errors = result["environment_errors"]
    session = session_witness(events, expected_pid if expected_pid is not None else evidence.get("pid"), errors)
    delivery_evidence(result, events, expected_pid if expected_pid is not None else evidence.get("pid"),
                      coverage_end_t=max((e["t"] for e in events if e.get("cat") != "log"
                                          and number(e.get("t")) is not None), default=0))
    errors.extend(cache_errors(events, evidence, case_id))
    errors.extend(static_request_errors(events, evidence, case_id))
    qa = [e for e in events if e.get("cat") == "qa" and e.get("case_id") == case_id]
    result["evidence"]["operations"] = evidence
    if case_id in ("static-foreground", "static-background"):
        result["metrics"]["requested_px"] = evidence.get("requested_px")
    exit_events = [e for e in qa if e.get("kind") == "on_exit" and e.get("flush_complete") is True]
    exit_external = evidence.get("exit", {})
    if not exit_events or exit_external.get("process_exited") is not True or exit_external.get("forced") is not False:
        errors.append("normal exit/flush/process termination not proven")
    seed_kinds = evidence.get("seed_kinds", [evidence["seed_kind"]] if evidence.get("seed_kind") else [])
    for seed_kind in seed_kinds:
        seeds = [e for e in qa if e.get("kind") == "seed" and e.get("seed_kind") == seed_kind
                 and e.get("normal_producer") is True]
        if not seeds:
            errors.append(f"normal {seed_kind} seed producer witness missing")
        if evidence.get("copied_after_process_exit") is not True:
            errors.append("seed was not copied after complete process termination")
        if seed_kind in ("cache", "video-pin"):
            key = evidence.get("target_key")
            if evidence.get("seed_phase", "preparation") == "reload":
                if not key or not any(e.get("cat") == "qa" and e.get("kind") == "cache_read"
                                      and e.get("key") == key and e.get("from_cache") is True for e in events):
                    errors.append("seed cache read after restart not proven for target key")
            elif seed_kind == "cache" and not any(e.get("saved_cache") is True and target_keys(e.get("targets", []))
                                                   for e in seeds):
                errors.append("normal cache seed durable save/target witness missing")
            elif seed_kind == "video-pin":
                cache_seeds = [e for e in qa if e.get("kind") == "seed" and e.get("seed_kind") == "cache"
                               and e.get("normal_producer") is True and e.get("saved_cache") is True]
                pinned = [e for e in seeds if e.get("pin_saved") is True and isinstance(e.get("key"), str)]
                if not any(f"dir::{Path(pin['key']).parent}" in target_keys(c.get("targets", []))
                           for pin in pinned for c in cache_seeds):
                    errors.append("normal video pin save plus parent-tile durable cache seed not proven")
        if seed_kind == "cache" and "seed_cache_paths" in evidence:
            saved = {normalize_search_path(e["folder_path"]) for e in seeds
                     if isinstance(e.get("folder_path"), str) and e.get("saved_cache") is True
                     and target_keys(e.get("targets", []))}
            intended = evidence["seed_cache_paths"]
            if (not isinstance(intended, list) or not intended
                    or any(not isinstance(p, str) or normalize_search_path(p) not in saved for p in intended)):
                errors.append("normal cache seed missing intended folder durable save witness")
        if seed_kind == "search":
            if not any(e.get("cat") == "index" and e.get("kind") == "initial_scan_settled" for e in events):
                errors.append("seed index initial scan did not settle")
            search = evidence.get("search", {})
            if (not search.get("expected_hits") or
                    {normalize_search_path(p) for p in search.get("expected_hits", [])}
                    != {normalize_search_path(p) for p in search.get("actual_hits", [])}
                    or search.get("completed") is not True):
                errors.append("seed search expected hit set not proven")
    if case_id.startswith("tray-close-"):
        deadline = number(setup_deadline_t)
        if deadline is None or deadline < 0:
            errors.append("tray converted measurement-start setup deadline missing/invalid")
            deadline = None

        def established_by_start(timestamp: float) -> bool:
            return deadline is not None and 0 <= timestamp <= deadline

        closes = [e for e in qa if e.get("kind") == "close_accept"]
        hides = [e for e in qa if e.get("kind") == "tray_hide"]
        accepted = None
        for close in closes:
            for hide in hides:
                if (uint(close.get("close_sequence"), positive=True)
                        and uint(close.get("items_gen"))
                        and uint(hide.get("close_sequence"), positive=True) and uint(hide.get("items_gen"))
                        and hide.get("close_sequence") == close["close_sequence"]
                        and hide.get("items_gen") == close.get("items_gen")
                        and hide.get("folder_path") == close.get("folder_path")
                        and isinstance(hide.get("folder_path"), str) and hide["folder_path"]
                        and hide.get("window_visible") is False
                        and number(close.get("t")) is not None and number(hide.get("t")) is not None
                        and established_by_start(close["t"]) and established_by_start(hide["t"])
                        and hide["t"] >= close["t"]):
                    accepted = (close, hide)
        if accepted is None or len(closes) != 1 or len(hides) != 1:
            errors.append("exactly one native close acceptance and correlated tray hide required")
        else:
            close, hide = accepted
            cache_check_ms = number(close.get("cache_check_unix_ms"))
            anchor_ms, anchor_t = number(session.get("wall_unix_ms")), number(session.get("wall_t"))
            cache_check_t = (anchor_t + (cache_check_ms - anchor_ms) / 1000
                             if cache_check_ms is not None and cache_check_ms > 0
                             and anchor_ms is not None and anchor_ms > 0 and anchor_t is not None and anchor_t >= 0 else None)
            if (cache_check_t is None or not established_by_start(cache_check_t) or cache_check_t > close["t"]
                    or hide.get("cache_check_unix_ms") != close.get("cache_check_unix_ms")):
                errors.append("tray cache-absence check clock missing/invalid or later than close acceptance")
            expected_complete = case_id == "tray-close-after-loading"
            if close.get("loading_complete") is not expected_complete or hide.get("loading_complete") is not expected_complete:
                errors.append("loading state at close acceptance/hide does not establish this case")
            valid_targets = []
            begins = request_begins(events)
            for target in hide.get("targets", []) if isinstance(hide.get("targets"), list) else []:
                if (not isinstance(target, dict) or target.get("catalog_absent") is not True
                        or target.get("cache_map_absent") is not True
                        or not isinstance(target.get("key"), str) or not target["key"] or not uint(target.get("idx"))
                        or not uint(target.get("items_gen"))
                        or target.get("items_gen") != hide.get("items_gen")):
                    continue
                accepted_targets = [t for t in close.get("targets", []) if isinstance(t, dict)
                                    and uint(t.get("idx")) and uint(t.get("items_gen"))
                                    and all(t.get(f) == target.get(f) for f in ("key", "idx", "items_gen"))
                                    and t.get("catalog_absent") is True and t.get("cache_map_absent") is True]
                if not accepted_targets:
                    continue
                # A unique owner ID is not inferred from seq: retries can reuse it.
                finalized = [e for e in qa if e.get("kind") == "finalized"
                             and all(e.get(f) == target.get(f) for f in ("key", "idx", "items_gen"))
                             and uint(e.get("close_sequence"), positive=True)
                             and e.get("close_sequence") == hide.get("close_sequence")
                             and number(e.get("t")) is not None and e["t"] > hide["t"]
                             and established_by_start(e["t"])
                             and e.get("image_backlog_contains_request") is True and e.get("window_visible") is False]
                for final in finalized:
                    identity = request_identity(final)
                    begin = begins.get(identity)
                    if begin is None or not hide["t"] < begin["t"] <= final["t"]:
                        continue
                    sources = [e for e in qa if e.get("kind") == "source"
                               and request_identity(e) == identity
                               and correlated_producer(e, begins) is not None
                               and e.get("origin") == "source_generated"
                               and begin["t"] <= e["t"] <= final["t"]
                               and established_by_start(e["t"])
                               and e.get("should_save") is False and e.get("force_cache") is False]
                    if any(e.get("kind") == "source" and request_identity(e) == identity
                           and (e.get("should_save") is True or e.get("force_cache") is True) for e in qa):
                        continue
                    if sources:
                        source = min(sources, key=lambda e: e["t"])
                        valid_targets.append({**target, "request_id": identity[0], "finalized_t": final["t"],
                                              "request_begin_t": begin["t"], "source_t": source["t"],
                                              "save_skipped_t": source["t"], "origin": source["origin"]})
            if not valid_targets:
                errors.append("tray save-skip/cache absence/post-hide source/Finalized-image-backlog request correlation missing by measurement start")
            result["evidence"]["tray"] = {"close": close, "hide": hide, "valid_targets": valid_targets,
                                           "setup_deadline_t": deadline, "cache_check_t": cache_check_t}
    return finish(result)


def analyze_idle_case(events: list[dict], sampler: dict, evidence: dict,
                      thresholds: dict, expected_pid: int | None) -> dict:
    """Combine the established idle counter with the stricter suite setup contract."""
    result = convert_clock_window(events, sampler, expected_pid)
    result["case_id"] = evidence.get("case_id", sampler.get("case_id", "idle"))
    clock = result["evidence"].get("clock")
    clock_valid = clock is not None and not result["environment_errors"]
    witnesses = analyze_case_evidence(events, evidence, expected_pid,
                                     setup_deadline_t=clock["start_t"] if clock else None)
    result["environment_errors"].extend(witnesses["environment_errors"])
    result["evidence"].update(witnesses["evidence"])
    result["metrics"].update(witnesses["metrics"])
    # External CPU/log measurements and the sampler verdict do not depend on
    # mapping UTC to perf time. Preserve them even when that mapping is invalid.
    result["environment_errors"].extend(sampler.get("environment_errors", []))
    result["warnings"].extend(sampler.get("warnings", []))
    if sampler.get("verdict") == "PASS_WITH_WARNINGS" and not sampler.get("warnings"):
        result["warnings"].append("external sampler reported warnings (details absent)")
    if sampler.get("verdict") == "FAIL":
        result["failures"].extend(sampler.get("failures") or ["external sampler FAIL"])
    for field, limit in (("cpu_core_ratio", "max_cpu_core_ratio"),
                         ("app_log_growth_bytes", "max_app_log_growth_bytes"),
                         ("perf_log_growth_bytes", "max_perf_log_growth_bytes")):
        value = number(sampler.get(field))
        if value is None or value < 0:
            result["environment_errors"].append(f"missing/invalid external {field}")
        elif value > thresholds[limit]:
            result["failures"].append(f"{field} {value} exceeds {thresholds[limit]}")
        elif field == "cpu_core_ratio" and value > thresholds["target_cpu_core_ratio"]:
            result["warnings"].append(f"CPU one-core ratio {value} exceeds target {thresholds['target_cpu_core_ratio']}")
        result["metrics"][field] = value
    if not clock_valid:
        return finish(result)
    if sampler.get("index_scan_wait") != "settled":
        result["environment_errors"].append("fixture index initial scan completion not confirmed")
    if sampler.get("identity_evidence") != "command_line" or sampler.get("portable_identity_verified") is not True:
        result["environment_errors"].append("exact portable exe/data/command-line identity missing")
    if abs(clock["wall_seconds"] - thresholds["measure_seconds"]) > .25:
        result["environment_errors"].append("external measurement duration does not match threshold window")
    if sampler.get("process_alive_start") is not True or sampler.get("process_alive_end") is not True:
        result["environment_errors"].append("process lifetime at both measurement boundaries not confirmed")
    samples = sampler.get("window_samples", [])
    if (not isinstance(samples, list) or len(samples) < 2 or
            any(not isinstance(s, dict) or number(s.get("unix_ms")) is None for s in samples)):
        result["environment_errors"].append("foreground/window lifetime samples missing or invalid")
    else:
        samples = sorted(samples, key=lambda s: s["unix_ms"])
        max_gap = max(b["unix_ms"] - a["unix_ms"] for a, b in zip(samples, samples[1:]))
        result["metrics"]["window_sample_max_gap_ms"] = max_gap
        if (samples[0]["unix_ms"] > clock["measure_start_unix_ms"] + 100 or
                samples[-1]["unix_ms"] < clock["measure_end_unix_ms"] - 100):
            result["environment_errors"].append("lifetime samples do not cover the complete measurement window")
        if max_gap > 1100:
            result["environment_errors"].append(
                f"lifetime sample gap {max_gap} ms exceeds 1100 ms coverage limit")
        if result["case_id"] == "static-foreground":
            root_hwnd = sampler.get("root_hwnd")
            if (not isinstance(root_hwnd, int) or root_hwnd <= 0 or
                    any(s.get("foreground_pid") != expected_pid or s.get("foreground_hwnd") != root_hwnd for s in samples)):
                result["environment_errors"].append("exact App PID was not foreground throughout measurement")
        elif result["case_id"].endswith("background"):
            background_hwnd, background_pid = sampler.get("background_hwnd"), sampler.get("background_process_id")
            if (not isinstance(background_hwnd, int) or background_hwnd <= 0
                    or not isinstance(background_pid, int) or background_pid <= 0 or background_pid == expected_pid
                    or any(s.get("foreground_pid") != background_pid or s.get("foreground_hwnd") != background_hwnd
                           for s in samples)):
                result["environment_errors"].append("App background state not proven throughout measurement")
        elif result["case_id"].startswith("tray-close-"):
            if any(not isinstance(s.get("total"), int) or s["total"] < 1
                   or s.get("visible") != 0 for s in samples):
                result["environment_errors"].append("tray hidden top-level window lifetime not proven")
    idle = analyze_idle_health(
        events, clock["start_t"], clock["end_t"], expected_pid=expected_pid,
        allow_sleeping_window=True, require_work_key=evidence.get("target_key"),
        target_update_rate=thresholds["target_update_rate_per_sec"],
        max_update_rate=thresholds["max_update_rate_per_sec"],
        max_reason_streak_secs=thresholds["max_reason_streak_seconds"],
        max_same_work=thresholds["max_same_work"], max_input_events=thresholds["max_input_events"],
    )
    result["metrics"]["idle"] = idle["metrics"]
    result["evidence"]["idle_delivery"] = idle["delivery"]
    if idle["status"] == "CannotJudge":
        result["environment_errors"].extend(f"idle log delivery: {reason}" for reason in idle["delivery"]["reasons"])
    result["failures"].extend(idle["failures"])
    result["warnings"].extend(idle["warnings"])
    if result["case_id"] == "video-pin-background":
        key = evidence.get("target_key")
        latest_open = max((e.get("t", 0) for e in events if e.get("cat") == "nav"
                           and e.get("kind") == "load_folder_begin" and e.get("t", 0) <= clock["end_t"]), default=0)
        if not key or not any(e.get("cat") == "qa" and e.get("kind") == "cache_read"
                              and e.get("key") == key and e.get("from_cache") is True
                              and latest_open <= e.get("t", 0) <= clock["end_t"] for e in events):
            result["environment_errors"].append("video pin cached read since latest folder load missing")
        if not key or not any(e.get("cat") == "thumb" and e.get("kind") in
                              ("idle_upgrade_enqueue", "idle_upgrade_ineligible") and e.get("key") == key
                              and latest_open <= e.get("t", 0) <= clock["end_t"] for e in events):
            result["environment_errors"].append("video pin idle upgrade evaluation missing")
    return finish(result)


def summarize_suite(cases: list[dict], metadata: dict | None = None) -> dict:
    result = report("release-checks")
    if metadata is not None and not isinstance(metadata, dict):
        result["environment_errors"].append("run metadata must be an object")
        metadata = {}
    result["metadata"] = metadata or {}
    result["cases"] = cases
    for field in ("source", "exe_sha256", "fixture_version", "environment", "run_path", "approval_scope", "thresholds_version"):
        if not result["metadata"].get(field):
            result["environment_errors"].append(f"required run metadata missing: {field}")
    ids = [c.get("case_id") for c in cases]
    for case_id in REQUIRED_CASES:
        if ids.count(case_id) != 1:
            result["environment_errors"].append(f"required case missing or duplicated: {case_id}")
    for case in cases:
        case_id, verdict = case.get("case_id", "?"), case.get("verdict")
        if not isinstance(case_id, str):
            result["environment_errors"].append("case_id must be a string")
        for field in ("failures", "environment_errors", "warnings"):
            value = case.get(field, [])
            if not isinstance(value, list) or not all(isinstance(v, str) for v in value):
                result["environment_errors"].append(f"invalid case {field} list: {case_id}")
        for field in ("metrics", "evidence"):
            if field in case and not isinstance(case[field], dict):
                result["environment_errors"].append(f"invalid case {field} object: {case_id}")
        if verdict in ("PASS", "PASS_WITH_WARNINGS") and case.get("schema_version") != 1:
            result["environment_errors"].append(f"unsupported case report schema: {case_id}")
        expected_code = {"FAIL": 1, "SKIPPED": 2, "PASS": 0, "PASS_WITH_WARNINGS": 0}.get(verdict) if isinstance(verdict, str) else None
        if (expected_code is None or not isinstance(case.get("exit_code"), int)
                or isinstance(case.get("exit_code"), bool) or case.get("exit_code") != expected_code):
            result["environment_errors"].append(f"invalid case verdict/exit_code: {case_id}")
        if verdict == "FAIL" or case.get("failures"):
            result["failures"].append(f"{case_id}: {case.get('failures') or 'FAIL'}")
        if verdict == "SKIPPED" or case.get("environment_errors"):
            result["environment_errors"].append(f"{case_id}: {case.get('environment_errors') or 'SKIPPED'}")
        if verdict == "PASS_WITH_WARNINGS" or case.get("warnings"):
            result["warnings"].append(f"{case_id}: {case.get('warnings') or 'warnings'}")
    if result["metadata"].get("include_first_ai") is True:
        ai_cases = [c for c in cases if c.get("case_id") == "first-ai"]
        ai_evidence = ai_cases[0].get("evidence") if len(ai_cases) == 1 else None
        if (not isinstance(ai_evidence, dict) or not isinstance(ai_evidence.get("ai"), dict)
                or ai_evidence["ai"].get("included") is not True):
            result["environment_errors"].append("explicit first AI inclusion was not performed in its independent session")
    result["metrics"]["case_verdicts"] = dict(Counter(str(c.get("verdict", "invalid")) for c in cases))
    return finish(result)


def release_record(result: dict) -> str:
    """A pasteable record; never append to the authoritative release history."""
    meta = result.get("metadata", {})
    lines = [f"- **release-check automation (隔離診断 portable)**: {result['verdict']}。",
             f"  source {meta.get('source', '未記録')}、artifact {meta.get('exe_sha256', '未記録')}、"
             f"release + portable,test-script、fixture {meta.get('fixture_version', '未記録')}。",
             f"  環境 {meta.get('environment', '未記録')}、run {meta.get('run_path', '未記録')}、"
             f"了承範囲 {meta.get('approval_scope', '未記録')}、閾値 {meta.get('thresholds_version', '未記録')}。"]
    for case in result.get("cases", []):
        lines.append(f"- **{case.get('case_id', '?')}**: {case.get('verdict', 'SKIPPED')}。")
        metrics = case.get("metrics") if isinstance(case.get("metrics"), dict) else {}
        evidence = case.get("evidence") if isinstance(case.get("evidence"), dict) else {}
        if "drawn" in metrics:
            draw, pg = metrics["drawn"], metrics["pre_grid"]
            operations = evidence.get("operations", {})
            if case.get("case_id") in ("perf-cached", "perf-uncached"):
                lines.append(f"  distinct image-folder loads {len(operations.get('folder_loads', []))}/256、"
                             f"Ctrl+Down {len(operations.get('navigation', []))}/5、"
                             f"fixture search completed={operations.get('search', {}).get('completed', False)}。")
            ratio = draw.get("below_16ms_ratio")
            percent = f"{ratio * 100:.2f}%" if ratio is not None else "n/a"
            lines.append(f"  total frames {metrics['total_frames']}、drawn <16ms "
                         f"{draw['below_16ms']}/{draw['n']} ({percent}) / p95 {draw['p95_ms']}ms / max {draw['max_ms']}ms、"
                         f"pre_grid raw p50 / p95 / max {pg['p50_ms']} / {pg['p95_ms']} / {pg['max_ms']}ms。")
            lines.append(f"  100ms超 gaps {metrics['gap_count']} / updates {metrics['slow_update_count']}、"
                         f"gap分類 {json.dumps(metrics['gap_categories'], ensure_ascii=False)}、"
                         f"update分類 {json.dumps(metrics['update_categories'], ensure_ascii=False)}、"
                         f"unexplained {metrics['unexplained_count']}、AI "
                         f"{evidence.get('ai', {}).get('status', '明示実施')}。")
            lines.append("  known形の最大値 / scan / viewport " + json.dumps(
                {k: v for k, v in metrics.get("classifications", {}).items() if k.startswith("known-")},
                ensure_ascii=False) + "。")
            for gap in evidence.get("gaps", []):
                if "known-startup-first-frame" in gap.get("categories", []):
                    lines.append(f"  known-startup-first-frame: frame 1→2、{gap['gap_ms']:.3f}ms "
                                 f"(t={gap['start_t']}→{gap['end_t']}、上限 {gap['upper_bound_ms']}ms、"
                                 f"最初の測定操作 t={gap['first_operation_t']} より前、利用者了承2026-10-11)。")
            for update in evidence.get("slow_updates", []):
                reason = f"slow update {update['n']} lacks stage breakdown"
                if reason in case.get("environment_errors", []):
                    lines.append(f"  環境注記: {reason}、{update['total_ms']:.3f}ms "
                                 f"(t={update['start_t']}→{update['end_t']})。段別証拠不足の環境不成立を維持。")
        if metrics and "drawn" not in metrics:
            lines.append("  metrics " + json.dumps(metrics, ensure_ascii=False) + "。")
        clock = evidence.get("clock")
        if clock:
            lines.append("  sampler UTC / session anchor / perf window / sleep包含 "
                         + json.dumps(clock, ensure_ascii=False) + "。")
        for field in ("failures", "environment_errors", "warnings"):
            if case.get(field):
                details = case[field]
                text = "; ".join(details) if isinstance(details, list) and all(isinstance(v, str) for v in details) else json.dumps(details, ensure_ascii=False)
                lines.append(f"  {field}: " + text)
    if result.get("environment_errors"):
        lines.append("- **未完了 / 環境不成立**: " + "; ".join(result["environment_errors"]))
    lines.append("- **手動確認**: 実データ・実タイトルバー[×] / tray復帰・native描画 / IME / DPI は未実施。"
                 "診断portableを配布coreの実測とは扱わない。tray旧版FAIL / 修正版PASS採用証拠は別途必要。")
    return "\n".join(lines) + "\n"


def write_report(result: dict, path: Path | None) -> None:
    if path:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(result, ensure_ascii=False, indent=2, allow_nan=False) + "\n", encoding="utf-8")
    print(json.dumps(result, ensure_ascii=False, allow_nan=False))


def add_perf_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--expected-pid", type=int)
    parser.add_argument("--case-id", default="perf-smoke")
    parser.add_argument("--case-evidence", type=Path)
    parser.add_argument("--fixture-manifest", type=Path)
    parser.add_argument("--json-out", type=Path)


def perf_smoke_cli(args: argparse.Namespace) -> int:
    events, errors = load_events_strict(args.jsonl)
    evidence = {}
    if args.case_evidence:
        try:
            evidence = read_json(args.case_evidence)
        except (OSError, ValueError) as exc:
            errors.append(f"cannot read case evidence: {exc}")
    if getattr(args, "fixture_manifest", None):
        try:
            evidence["fixture_manifest"] = read_json(args.fixture_manifest)
        except (OSError, ValueError) as exc:
            errors.append(f"cannot read fixture manifest: {exc}")
    try:
        result = analyze_perf_smoke(events, expected_pid=args.expected_pid, case_id=args.case_id,
                                    evidence=evidence, load_errors=errors)
    except (ValueError, TypeError, AttributeError, KeyError) as exc:
        result = report(args.case_id)
        result["environment_errors"].extend(errors + [f"invalid perf/evidence fields: {exc}"])
        finish(result)
    write_report(result, args.json_out)
    return result["exit_code"]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    perf = sub.add_parser("perf-smoke")
    perf.add_argument("jsonl", type=Path)
    add_perf_arguments(perf)
    clock = sub.add_parser("clock-window")
    clock.add_argument("jsonl", type=Path)
    clock.add_argument("--sampler", required=True, type=Path)
    clock.add_argument("--expected-pid", type=int)
    clock.add_argument("--json-out", type=Path)
    check = sub.add_parser("evidence")
    check.add_argument("case_evidence", type=Path)
    check.add_argument("--jsonl", required=True, type=Path)
    check.add_argument("--expected-pid", type=int)
    check.add_argument("--json-out", type=Path)
    idle = sub.add_parser("idle-case")
    idle.add_argument("jsonl", type=Path)
    idle.add_argument("--case-id", required=True)
    idle.add_argument("--expected-pid", type=int, required=True)
    idle.add_argument("--sampler", required=True, type=Path)
    idle.add_argument("--case-evidence", required=True, type=Path)
    idle.add_argument("--thresholds", type=Path, default=Path(__file__).with_name("idle_health_thresholds.json"))
    idle.add_argument("--json-out", type=Path)
    summary = sub.add_parser("summary")
    summary.add_argument("suite_input", type=Path)
    summary.add_argument("--json-out", type=Path)
    summary.add_argument("--markdown-out", type=Path)
    args = parser.parse_args(argv)
    if args.command == "perf-smoke":
        return perf_smoke_cli(args)
    try:
        if args.command == "summary":
            inputs = read_json(args.suite_input)
            raw_cases = inputs.get("cases", [])
            if not isinstance(raw_cases, list):
                raise ValueError("suite cases must be a list")
            cases = []
            for i, value in enumerate(raw_cases):
                try:
                    case = read_json(args.suite_input.parent / value) if isinstance(value, str) else value
                    if not isinstance(case, dict):
                        raise ValueError("case must be an object or report path")
                except (OSError, ValueError, TypeError) as exc:
                    case = report(f"invalid-case-{i}")
                    case["environment_errors"].append(f"cannot read case {value}: {exc}")
                    finish(case)
                cases.append(case)
            result = summarize_suite(cases, inputs.get("metadata", {}))
        else:
            events, errors = load_events_strict(args.jsonl)
            if args.command == "clock-window":
                result = convert_clock_window(events, read_json(args.sampler), args.expected_pid)
            elif args.command == "idle-case":
                evidence = read_json(args.case_evidence)
                evidence["case_id"] = args.case_id
                result = analyze_idle_case(events, read_json(args.sampler), evidence,
                                           read_json(args.thresholds), args.expected_pid)
            else:
                result = analyze_case_evidence(events, read_json(args.case_evidence), args.expected_pid)
            result["environment_errors"].extend(errors)
            finish(result)
    except (OSError, ValueError, TypeError, AttributeError, KeyError) as exc:
        result = report("release-checks" if args.command == "summary" else args.command)
        result["environment_errors"].append(f"invalid/missing input: {exc}")
        finish(result)
    write_report(result, args.json_out)
    if args.command == "summary" and args.markdown_out:
        args.markdown_out.parent.mkdir(parents=True, exist_ok=True)
        args.markdown_out.write_text(release_record(result), encoding="utf-8")
    return result["exit_code"]


if __name__ == "__main__":
    raise SystemExit(main())
