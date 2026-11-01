#!/usr/bin/env python3
"""試作 D 記録: callback 内の lead を集計する。デバイス/描画遅延は測らない。

Usage: python scripts/analyze-lyrics-trial.py perf.jsonl [--scenes scenes.json]
scenes.json: [{"name":"通常再生", "start":10.0, "end":20.0}, ...]
時刻は perf の t (秒)。シーク/速度変更/遅延変化/DSP引継ぎ/EOF・ループの
操作区間を T3 で指定する。指定のない行は「未分類」。場面を推測して診断しない。
"""
import argparse
from collections import Counter, defaultdict
import json
import math
from pathlib import Path


def percentile(values, fraction):
    ordered = sorted(values)
    position = (len(ordered) - 1) * fraction
    lower = math.floor(position)
    upper = math.ceil(position)
    return ordered[lower] + (ordered[upper] - ordered[lower]) * (position - lower)


def analyze(rows, scenes=()):
    values = defaultdict(list)
    excluded = defaultdict(Counter)
    previous = {}  # Dedicated diagnostics belong to the media generation, never the host incarnation.
    for row in rows:
        if row.get("cat") != "effetune" or row.get("kind") != "lyrics_position":
            continue
        scene = next((s["name"] for s in scenes if s["start"] <= row["t"] < s["end"]), "未分類")
        reason = None
        generation = row["media_generation"]
        seq = row.get("lead_seq")
        if not row.get("available"):
            reason = "位置なし"
        elif row.get("lead_status") == "not_sampled":
            reason = "未採取"
        elif not row.get("lead_coherent") or not isinstance(seq, int) or seq == 0 or seq % 2:
            reason = "読み取り中の更新"
        else:
            old = previous.get(generation, 0)
            previous[generation] = seq
            if seq <= old:
                reason = "連番が進んでいない"
            elif not row.get("playing"):
                reason = "一時停止・停止"
            elif not (row["lead_pcm_serial"] == row["lead_clock_serial_before"] == row["lead_clock_serial_after"]):
                reason = "採取中のserial不一致"
            elif row["lead_pcm_serial"] != row["seek_serial"]:
                reason = "旧いseek serial"
            elif not isinstance(row.get("lead_ms"), (int, float)) or not math.isfinite(row["lead_ms"]):
                reason = "不正なlead"
        if reason:
            excluded[scene][reason] += 1
        else:
            values[scene].append(row["lead_ms"])
    return {
        name: {
            "accepted": len(values[name]),
            "lead_ms": ({"median": percentile(values[name], .5), "p95": percentile(values[name], .95),
                         "abs_p95": percentile([abs(v) for v in values[name]], .95),
                         "min": min(values[name]), "max": max(values[name])} if values[name] else None),
            "excluded": dict(excluded[name]),
        }
        for name in sorted(values.keys() | excluded.keys())
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("perf", type=Path)
    parser.add_argument("--scenes", type=Path)
    args = parser.parse_args()
    scenes = json.loads(args.scenes.read_text(encoding="utf-8-sig")) if args.scenes else []
    for scene in scenes:
        if not scene["start"] < scene["end"]:
            parser.error("scene start must be earlier than end")
    with args.perf.open(encoding="utf-8-sig") as stream:
        result = analyze((json.loads(line) for line in stream if line.strip()), scenes)
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0 if result else 2


if __name__ == "__main__":
    raise SystemExit(main())
