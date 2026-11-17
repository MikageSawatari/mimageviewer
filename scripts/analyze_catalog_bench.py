#!/usr/bin/env python3
"""Summarize bench_scroll --synthetic-suite results without hiding slow cases.

The strict default comparison has no timing tolerance. A design owner may pass
an explicit noise budget; the report records it. These headless measurements do
not establish cold-OS-cache performance or replace the user's runtime perf log.
"""
from __future__ import annotations

import argparse
import json
import statistics
from pathlib import Path


def summarize(rows: list[dict], noise_ms: float = 0.0, relative: float = 0.0) -> dict:
    protocol = [row for row in rows if row.get("kind") == "protocol"]
    if len(protocol) != 1 or protocol[0].get("version") != 1:
        raise ValueError("Exactly one version-1 protocol row is required")
    protocol = protocol[0]
    expected_repetitions = protocol["repetitions"]
    groups: dict[tuple, dict[str, list[dict]]] = {}
    for row in rows:
        if row.get("kind") not in ("catalog", "seed"):
            continue
        group = (row["kind"], row.get("background_rows"), row["shape"],
                 row["connection"], row.get("workers"))
        groups.setdefault(group, {}).setdefault(row["mode"], []).append(row)
    comparisons = []
    errors = []
    for group, modes in sorted(groups.items(), key=lambda item: str(item[0])):
        kind, background, shape, connection, workers = group
        metrics = ["ui_prepare_ms"]
        if kind == "catalog":
            metrics += ["initial_ready_ms", "initial.all_visible_ms", "scroll.all_visible_ms"]
        else:
            metrics += ["seed_ms"]
        expected_modes = ("memory", "db", "source") if kind == "catalog" else ("memory", "db")
        for mode in expected_modes:
            records = modes.get(mode, [])
            if len(records) != expected_repetitions or {r["repetition"] for r in records} != set(range(expected_repetitions)):
                errors.append(f"Incomplete or duplicated group: {group}, mode={mode}")
        if "memory" not in modes or "db" not in modes:
            continue

        def value(row: dict, metric: str) -> float:
            result = row
            for part in metric.split("."):
                result = result[part]
            return float(result)

        medians = {
            mode: {metric: statistics.median(value(row, metric) for row in records) for metric in metrics}
            for mode, records in modes.items()
        }
        deltas = {metric: medians["db"][metric] - medians["memory"][metric] for metric in metrics}
        comparisons.append({"kind": kind, "background_rows": background, "shape": shape,
                            "connection": connection, "workers": workers, "medians": medians,
                            "db_minus_memory_ms": deltas})

    # Require every planned shape/connection/worker group. Never infer a pass
    # from a truncated run or from an unexercised fast subset.
    workers = protocol["worker_counts"]
    catalog_groups = [r for r in comparisons if r["kind"] == "catalog"]
    seed_groups = [r for r in comparisons if r["kind"] == "seed"]
    expected_catalog = {(shape, connection, worker)
                        for shape in ("full", "empty", "stale90", "partial50")
                        for connection in ("fresh", "warm") for worker in workers}
    actual_catalog = {(r["shape"], r["connection"], r["workers"]) for r in catalog_groups}
    if not workers or 1 not in workers or actual_catalog != expected_catalog:
        errors.append("Missing required catalog shape/connection/low-worker coverage")
    expected_seed = {(background, shape, connection) for background in (0, 10_000)
                     for shape in ("empty", "same", "partial") for connection in ("fresh", "warm")}
    if {(r["background_rows"], r["shape"], r["connection"]) for r in seed_groups} != expected_seed:
        errors.append("Missing required seed background/state/connection coverage")

    def no_slower(row: dict, metric: str) -> bool:
        old = row["medians"]["memory"][metric]
        return row["medians"]["db"][metric] <= old + noise_ms + old * relative

    full = [r for r in catalog_groups if r["shape"] == "full"]
    gates = {
        "1_full_10k_ui_preparation_shorter": bool(full) and all(r["db_minus_memory_ms"]["ui_prepare_ms"] < 0 for r in full),
        "2_initial_visible_no_slower": bool(catalog_groups) and all(no_slower(r, "initial_ready_ms") for r in catalog_groups),
        "3_200_pin_ui_preparation_no_slower": bool(seed_groups) and all(no_slower(r, "ui_prepare_ms") for r in seed_groups),
    }
    return {"protocol": protocol, "noise_ms": noise_ms, "relative_tolerance": relative,
            "evidence_errors": errors, "gates": gates, "pass": not errors and all(gates.values()),
            "comparisons": comparisons}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("results", type=Path)
    parser.add_argument("--noise-ms", type=float, default=0.0)
    parser.add_argument("--relative-tolerance", type=float, default=0.0)
    parser.add_argument("--json-out", type=Path)
    args = parser.parse_args()
    if args.noise_ms < 0 or args.relative_tolerance < 0:
        parser.error("Timing tolerances must be nonnegative")
    rows = [json.loads(line) for line in args.results.read_text(encoding="utf-8-sig").splitlines() if line.strip()]
    report = summarize(rows, args.noise_ms, args.relative_tolerance)
    for row in report["comparisons"]:
        label = f"{row['kind']} background={row['background_rows']} {row['shape']} {row['connection']} workers={row['workers']}"
        print(label)
        for metric, delta in row["db_minus_memory_ms"].items():
            print(f"  {metric}: memory={row['medians']['memory'][metric]:.3f} db={row['medians']['db'][metric]:.3f} delta={delta:+.3f} ms")
    print(json.dumps({key: value for key, value in report.items() if key != "comparisons"}, ensure_ascii=False, indent=2))
    if args.json_out:
        args.json_out.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    raise SystemExit(0 if report["pass"] else 1)


if __name__ == "__main__":
    main()
