#!/usr/bin/env python3
"""Classify typed foreground-wait failures from saved evidence only.

Only a healthy exact root and proven external foreground throughout the bounded
wait interval permit environment classification. Missing evidence remains FAIL.
This module never probes windows, starts processes, or writes evidence.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path

MAX_GAP_MS = 1500
FAILURE_FILE = "foreground-wait-failure.json"
TIMELINE_FILE = "foreground-timeline.jsonl"
WAIT_KINDS = {"exact-root-foreground", "ctrl-g-search-focus"}


def positive_int(value: object) -> bool:
    return type(value) is int and value > 0


def text_value(value: object) -> bool:
    return isinstance(value, str) and bool(value.strip())


def classify_foreground_wait_failure(failure: object, rows: object,
                                     expected_pid: int, case_id: str) -> dict:
    """Return a conservative decision; ``environment`` alone permits SKIPPED.

    Sampling is bounded by MAX_GAP_MS, including the samples bracketing the
    complete wait. Both boundary samples are checked even when outside the wait.
    """
    failure = failure if isinstance(failure, dict) else {}
    rows = rows if isinstance(rows, list) else []
    details = {
        "row_count": len(rows), "selected_row_count": 0,
        "wait_start_unix_ms": failure.get("wait_start_unix_ms"),
        "failure_unix_ms": failure.get("failure_unix_ms"),
        "max_gap_ms": MAX_GAP_MS,
        "wait_kind": failure.get("wait_kind"),
        "foreground": [],
    }
    # Preserve available identities even when validation or interval coverage fails.
    def identities(values: list) -> list[dict]:
        result = []
        for row in values:
            if isinstance(row, dict):
                identity = {field: row.get(field) for field in (
                    "foreground_pid", "foreground_process_name", "foreground_class",
                    "ghost_original_pid", "ghost_original_hwnd", "ghost_original_process_name")}
                if identity not in result:
                    result.append(identity)
        return result

    details["foreground"] = identities(rows)

    def decision(reason: str, environment: bool = False) -> dict:
        names = "; ".join(
            f"{entry['foreground_process_name']} (pid={entry['foreground_pid']}, "
            f"class={entry['foreground_class']})"
            + (f" original={entry['ghost_original_process_name']} "
               f"(pid={entry['ghost_original_pid']}, hwnd={entry['ghost_original_hwnd']})"
               if entry["foreground_class"] == "Ghost" else "")
            for entry in details["foreground"])
        summary = (f"{reason}; rows={details['row_count']}, "
                   f"selected={details['selected_row_count']}, "
                   f"wait={details['wait_start_unix_ms']}..{details['failure_unix_ms']}; "
                   f"kind={details['wait_kind']}; "
                   f"foreground={names or 'unavailable'}")
        return {"schema_version": 1, "case_id": case_id, "environment": environment,
                "reason": reason, "summary": summary, "details": details}

    if not positive_int(expected_pid):
        return decision("invalid expected application PID")
    if type(failure.get("schema_version")) is not int or failure.get("schema_version") != 1:
        return decision("missing or unsupported foreground-wait failure record")
    if type(failure.get("pid")) is not int or failure.get("pid") != expected_pid:
        return decision("foreground-wait failure PID does not match exact application PID")
    root = failure.get("root_hwnd")
    if not positive_int(root):
        return decision("foreground-wait failure has no exact root HWND")
    if not isinstance(failure.get("wait_kind"), str) or failure["wait_kind"] not in WAIT_KINDS:
        return decision("unsupported foreground-wait failure kind")
    start, end = failure.get("wait_start_unix_ms"), failure.get("failure_unix_ms")
    if not positive_int(start) or not positive_int(end) or end < start:
        return decision("invalid foreground-wait failure interval")
    if not rows:
        return decision("foreground timeline is missing or empty")
    previous = None
    for i, row in enumerate(rows, 1):
        if not isinstance(row, dict) or type(row.get("schema_version")) is not int or row.get("schema_version") != 1:
            return decision(f"invalid foreground timeline schema at row {i}")
        timestamp = row.get("unix_ms")
        if not positive_int(timestamp) or (previous is not None and timestamp <= previous):
            return decision(f"foreground timeline timestamps are not strictly ordered at row {i}")
        previous = timestamp
    before = [i for i, row in enumerate(rows) if row["unix_ms"] <= start]
    after = [i for i, row in enumerate(rows) if row["unix_ms"] >= end]
    if not before or not after:
        return decision("foreground timeline does not cover the whole wait interval")
    first, last = before[-1], after[0]
    selected = rows[first:last + 1]
    details.update(selected_row_count=len(selected),
                   coverage_start_unix_ms=selected[0]["unix_ms"],
                   coverage_end_unix_ms=selected[-1]["unix_ms"])
    details["foreground"] = identities(selected)
    for left, right in zip(selected, selected[1:]):
        if right["unix_ms"] - left["unix_ms"] > MAX_GAP_MS:
            return decision(f"foreground timeline gap exceeds {MAX_GAP_MS} ms")
    for i, row in enumerate(selected, first + 1):
        if (type(row.get("app_pid")) is not int or row.get("app_pid") != expected_pid
                or type(row.get("root_pid")) is not int or row.get("root_pid") != expected_pid
                or type(row.get("root_hwnd")) is not int or row.get("root_hwnd") != root):
            return decision(f"exact application/root ownership mismatch at row {i}")
        if (row.get("root_alive") is not True or row.get("root_visible") is not True
                or row.get("root_minimized") is not False or row.get("root_hung") is not False):
            return decision(f"application root is not alive, visible, restored and responsive at row {i}")
        if (not positive_int(row.get("foreground_hwnd")) or not positive_int(row.get("foreground_pid"))
                or not text_value(row.get("foreground_process_name")) or not text_value(row.get("foreground_class"))
                or row.get("ownership_resolved") is not True
                or type(row.get("foreground_descendant")) is not bool):
            return decision(f"foreground identity/ownership is unresolved at row {i}")
        if (row["foreground_pid"] == expected_pid or row["foreground_descendant"]
                or row["foreground_hwnd"] == root):
            return decision(f"application or descendant held foreground during the wait at row {i}")
        if row["foreground_class"].casefold() == "ghost":
            if row["foreground_process_name"].casefold() not in {"dwm", "dwm.exe"}:
                return decision(f"Ghost foreground owner is not resolved to DWM at row {i}")
            if (not positive_int(row.get("ghost_original_hwnd"))
                    or not positive_int(row.get("ghost_original_pid"))
                    or type(row.get("ghost_original_descendant")) is not bool
                    or not text_value(row.get("ghost_original_process_name"))):
                return decision(f"Ghost original window ownership is unresolved at row {i}")
            if (row["ghost_original_pid"] == expected_pid or row["ghost_original_descendant"]
                    or row["ghost_original_hwnd"] == root):
                return decision(f"application or descendant owns Ghost original window at row {i}")
    return decision("external foreground persisted throughout the healthy application wait", True)


def read_object(path: Path) -> dict:
    value = json.loads(path.read_text(encoding="utf-8-sig"),
                       parse_constant=lambda value: (_ for _ in ()).throw(ValueError(value)))
    if not isinstance(value, dict):
        raise ValueError("JSON record must be an object")
    return value


def classify_directory(directory: Path, expected_pid: int, case_id: str) -> dict:
    failure, rows, errors = {}, [], []
    try:
        failure = read_object(directory / FAILURE_FILE)
    except (OSError, UnicodeError, ValueError) as exc:
        errors.append(f"cannot read {FAILURE_FILE}: {type(exc).__name__}")
    try:
        with (directory / TIMELINE_FILE).open(encoding="utf-8-sig") as stream:
            for i, line in enumerate(stream, 1):
                if not line.strip() or not line.endswith("\n"):
                    errors.append(f"invalid foreground timeline line {i}")
                    continue
                try:
                    row = json.loads(line, parse_constant=lambda value: (_ for _ in ()).throw(ValueError(value)))
                    if not isinstance(row, dict):
                        raise ValueError("timeline row must be an object")
                    rows.append(row)
                except ValueError:
                    errors.append(f"invalid foreground timeline JSON at line {i}")
    except (OSError, UnicodeError) as exc:
        errors.append(f"cannot read {TIMELINE_FILE}: {type(exc).__name__}")
    result = classify_foreground_wait_failure(failure, rows, expected_pid, case_id)
    if errors:
        result.update(environment=False, reason=errors[0])
        result["summary"] = errors[0] + "; " + result["summary"]
        result["details"]["read_errors"] = errors
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", required=True, type=Path)
    parser.add_argument("--expected-pid", required=True, type=int)
    parser.add_argument("--case-id", required=True)
    args = parser.parse_args(argv)
    print(json.dumps(classify_directory(args.directory, args.expected_pid, args.case_id),
                     ensure_ascii=True, allow_nan=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
