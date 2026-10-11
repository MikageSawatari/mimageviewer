#!/usr/bin/env python3
"""Conservative §1.367 startup A/B evidence gate; never launches the application.

See docs/fts-startup-ab-measurement.md for the manifest and user-run procedure.
Exit 0 means every observed paired endpoint has no added startup time; it is
evidence for review, not an automatic decision to enable the opt-in feature.
Missing evidence, noisy/mixed deltas and regressions all return exit 1.
"""
from __future__ import annotations

import argparse
import itertools
import json
import math
from pathlib import Path
from statistics import median

from analyze_perf import load_events, validate_delivery

ENDPOINTS = ("indexer_init_result_send", "indexer_overlay_cleared",
             "metadata_search_ready", "metadata_supervisors_started")
CONDITIONS = tuple(itertools.product(("cold", "warm"), (False, True),
                                    ("zero", "few", "many"), ("idle", "contention")))


def finite_number(value):
    return type(value) in (int, float) and math.isfinite(value) and value >= 0


def one(events, kind):
    found = [e for e in events if e.get("cat") == "startup" and e.get("kind") == kind]
    if len(found) != 1:
        raise ValueError(f"expected one startup.{kind}, got {len(found)}")
    return found[0]


def read_run(run, base):
    events = load_events(base / run["log"])
    metrics = {}
    for kind in ENDPOINTS:
        event = one(events, kind)
        if not finite_number(event.get("t")):
            raise ValueError(f"invalid {kind}.t")
        if kind == "indexer_init_result_send" and (event.get("accepted") is not True or event.get("ready") is not True):
            raise ValueError("IndexerInit result not accepted/ready")
        if kind == "indexer_overlay_cleared" and event.get("ready") is not True:
            raise ValueError("overlay cleared without ready manager")
        metrics[kind] = event["t"] * 1000.0
    cleanup = one(events, "metadata_out_of_range_cleanup")
    enabled = run["arm"] == "B"
    if cleanup.get("integrity_feature_enabled") is not enabled or cleanup.get("success") is not True:
        raise ValueError("cleanup failed or feature arm not proved")
    if type(cleanup.get("owner_rows")) is not int or cleanup["owner_rows"] < 840000:
        raise ValueError("fixture has fewer than 840,000 inventory rows")
    if cleanup.get("rows") != 0:
        raise ValueError("normal fixture has ownership cleanup deletes")
    if one(events, "metadata_supervisors_started").get("integrity_feature_enabled") is not enabled:
        raise ValueError("supervisor feature arm mismatch")
    checks = [e for e in events if e.get("cat") == "startup" and e.get("kind") == "fts_integrity_check"]
    diagnostic = {"owner_rows": cleanup["owner_rows"]}
    if enabled:
        check = one(events, "fts_integrity_check")
        for name in ("owner_rows", "not_ok_rows", "ok_rows", "segments", "live_docs"):
            if type(check.get(name)) is not int or check[name] < 0:
                raise ValueError(f"invalid integrity count {name}")
            diagnostic[name] = check[name]
        if check.get("feature_enabled") is not True or check.get("missing_suspected") is not False:
            raise ValueError("B is not a normal, enabled integrity-check run")
        if check["owner_rows"] != cleanup["owner_rows"] or check["ok_rows"] + check["not_ok_rows"] != check["owner_rows"]:
            raise ValueError("inconsistent integrity counts")
        if check["live_docs"] != check["ok_rows"]:
            raise ValueError("normal fixture must have one live document per Ok row")
        if check["not_ok_rows"] != run["failed_rows"]:
            raise ValueError("Failed fixture count mismatch")
        if run["failed_population"] == "many" and check["not_ok_rows"] * 2 <= check["owner_rows"]:
            raise ValueError("many Failed rows must be a majority")
        if not finite_number(check.get("integrity_check_ms")):
            raise ValueError("missing integrity_check_ms")
        diagnostic["integrity_check_ms"] = check["integrity_check_ms"]
    elif checks:
        raise ValueError("A unexpectedly ran integrity check")
    measured = [one(events, kind) for kind in (*ENDPOINTS, "metadata_out_of_range_cleanup")]
    measured.extend(checks)
    if any(not finite_number(e.get("t")) for e in measured):
        raise ValueError("invalid measured event timestamp")
    coverage = max(e["t"] for e in measured)
    delivery = validate_delivery(events, coverage_end_t=coverage)
    if delivery["status"] != "complete":
        raise ValueError("log delivery not proved: " + "; ".join(delivery["reasons"]))
    return metrics, diagnostic


def distribution(values):
    ordered = sorted(values)
    return {"n": len(values), "min_ms": ordered[0], "median_ms": median(ordered),
            "p95_ms": ordered[math.ceil(.95 * len(ordered)) - 1], "max_ms": ordered[-1]}


def analyze(manifest, base):
    reasons, groups, diagnostics = [], {}, []
    if not isinstance(manifest, dict):
        return {"status": "not_adopt", "reasons": ["manifest must be an object"]}
    runs = manifest.get("runs", [])
    if not isinstance(runs, list):
        return {"status": "not_adopt", "reasons": ["manifest runs must be an array"]}
    if manifest.get("schema") != 1:
        reasons.append("manifest schema must be 1")
    for name in ("source_revision", "build_profile", "common_build_options", "fixture_preparation", "cache_protocol", "contention_protocol"):
        if not isinstance(manifest.get(name), str) or not manifest[name].strip():
            reasons.append(f"missing recorded {name}")
    resolution = manifest.get("measurement_resolution_ms")
    if not finite_number(resolution) or resolution < 1:
        reasons.append("record measurement_resolution_ms >= 1 (perf t is rounded to milliseconds; not a delay allowance)")
    for index, run in enumerate(runs):
        try:
            condition = (run["cache"], run["skip_offline_change_scan"], run["failed_population"], run["io"])
            if condition not in CONDITIONS or type(run["skip_offline_change_scan"]) is not bool:
                raise ValueError("unknown condition")
            if run["arm"] not in ("A", "B") or type(run["pair"]) is not int or run["pair"] < 1:
                raise ValueError("invalid arm/pair")
            failed = run["failed_rows"]
            if type(failed) is not int or failed < 0:
                raise ValueError("invalid failed_rows")
            if ((condition[2] == "zero" and failed != 0) or
                (condition[2] == "few" and not 0 < failed < 420000) or
                (condition[2] == "many" and failed <= 420000)):
                raise ValueError("Failed population does not match fixture")
            for name in ("fixture_id", "cache_evidence", "io_evidence", "binary_sha256"):
                if not isinstance(run.get(name), str) or not run[name].strip():
                    raise ValueError(f"missing {name}")
            metrics, diagnostic = read_run(run, base)
            entries = groups.setdefault(condition, {}).setdefault(run["pair"], [])
            entries.append((index, run, metrics))
            diagnostics.append({"run": index + 1, "arm": run["arm"], **diagnostic})
        except (KeyError, ValueError, TypeError, OSError) as error:
            reasons.append(f"run {index + 1}: {error}")
    summaries = []
    for condition in CONDITIONS:
        pairs = groups.get(condition, {})
        if len(pairs) < 3:
            reasons.append(f"{condition}: fewer than 3 repeated pairs")
        paired = []
        previous_order = None
        for number, entries in sorted(pairs.items()):
            if len(entries) != 2 or {e[1]["arm"] for e in entries} != {"A", "B"}:
                reasons.append(f"{condition} pair {number}: missing/duplicate arm")
                continue
            order = "".join(e[1]["arm"] for e in entries)
            if entries[1][0] != entries[0][0] + 1 or order == previous_order:
                reasons.append(f"{condition} pair {number}: require adjacent pairs in alternating AB/BA order")
            previous_order = order
            arms = {e[1]["arm"]: e for e in entries}
            for name in ("fixture_id", "failed_rows"):
                if arms["A"][1][name] != arms["B"][1][name]:
                    reasons.append(f"{condition} pair {number}: {name} differs")
            paired.append({kind: (arms["A"][2][kind], arms["B"][2][kind]) for kind in ENDPOINTS})
        for kind in ENDPOINTS:
            if not paired:
                continue
            a, b = [p[kind][0] for p in paired], [p[kind][1] for p in paired]
            delta = [bv - av for av, bv in zip(a, b)]
            # Zero is the acceptance boundary. Resolution is reported, never used
            # as a permitted regression. A mixed sign is inconclusive noise.
            verdict = "no_observed_added_time" if max(delta) <= 0 else ("regression" if min(delta) > 0 else "noisy_or_inconclusive")
            if verdict != "no_observed_added_time":
                reasons.append(f"{condition} {kind}: {verdict}")
            summaries.append({"condition": condition, "endpoint": kind, "A": distribution(a),
                              "B": distribution(b), "B_minus_A": distribution(delta), "verdict": verdict})
    return {"status": "not_adopt" if reasons else "evidence_ready_for_review",
            "measurement_resolution_ms": resolution, "reasons": reasons,
            "summary": summaries, "integrity_diagnostics": diagnostics}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        report = analyze(json.loads(args.manifest.read_text(encoding="utf-8-sig")), args.manifest.parent)
    except (OSError, ValueError) as error:
        report = {"status": "not_adopt", "reasons": [str(error)]}
    result = json.dumps(report, ensure_ascii=False, indent=2)
    if args.output:
        args.output.write_text(result + "\n", encoding="utf-8")
    print(result)
    return 0 if report["status"] == "evidence_ready_for_review" else 1


if __name__ == "__main__":
    raise SystemExit(main())
