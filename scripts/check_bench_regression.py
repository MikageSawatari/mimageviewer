#!/usr/bin/env python3
"""bench_search の JSON 出力を baseline と比較し、回帰を検出する。

Usage:
    # 最新計測を取って baseline と比較 (CI 風、デフォルト動作)
    cargo run --release --bin bench_search -- --docs 50000 --json /tmp/bench_new.json
    python scripts/check_bench_regression.py vendor/bench_baseline.json /tmp/bench_new.json

    # 初回登録 / リファレンスの更新
    python scripts/check_bench_regression.py --save vendor/bench_baseline.json /tmp/bench_new.json

判定:
- query 単位の `total_ms` が baseline 比 +THRESHOLD% (既定 30%) を超えたら回帰扱い (exit 1)。
- 新たに追加されたクエリは無視 (warning のみ)。baseline にあって新測定に無いクエリも warning。
- `hits` は変化を許容 (corpus 生成シードが固定なので変動は本来無いが、tantivy 側の微妙な
  順位変動で truncated 後の hits 数が動く可能性があり、性能 regression と無関係なため)。

CLAUDE.md のリリース手順 Phase 2 で実行する。CI が無いプロジェクトなので手動。
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

DEFAULT_THRESHOLD_PCT = 30.0


def load(path: Path) -> dict:
    if not path.exists():
        sys.exit(f"error: {path} が存在しません")
    with path.open("r", encoding="utf-8") as f:
        return json.load(f)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("baseline", type=Path, help="基準値の JSON (例: vendor/bench_baseline.json)")
    ap.add_argument("current", type=Path, help="今回計測の JSON (bench_search --json で生成)")
    ap.add_argument(
        "--threshold",
        type=float,
        default=DEFAULT_THRESHOLD_PCT,
        help=f"許容劣化率 (パーセント、既定 {DEFAULT_THRESHOLD_PCT})",
    )
    ap.add_argument(
        "--save",
        action="store_true",
        help="比較せず current を baseline にコピーする (初回登録 / リファレンス更新時)",
    )
    args = ap.parse_args()

    if args.save:
        cur = load(args.current)
        args.baseline.parent.mkdir(parents=True, exist_ok=True)
        with args.baseline.open("w", encoding="utf-8") as f:
            json.dump(cur, f, indent=2, ensure_ascii=False)
            f.write("\n")
        print(f"saved baseline: {args.baseline}")
        return 0

    base = load(args.baseline)
    cur = load(args.current)

    base_q = base.get("queries", {})
    cur_q = cur.get("queries", {})

    if not base_q:
        sys.exit("error: baseline.queries が空 (初回は --save で登録してください)")

    failures: list[str] = []
    warnings: list[str] = []

    def numeric_total_ms(record: dict, label: str, side: str) -> float | None:
        """`total_ms` が float として取れない (missing / 非数値 / NaN / Inf) ならエラー扱い。"""
        if "total_ms" not in record:
            failures.append(f"  {side}.{label}: total_ms フィールドが無い")
            return None
        try:
            v = float(record["total_ms"])
        except (TypeError, ValueError):
            failures.append(f"  {side}.{label}: total_ms が数値ではない ({record['total_ms']!r})")
            return None
        # NaN / Inf は JSON 仕様上は無効、Python は読み込めるが比較で罠になる。
        import math
        if math.isnan(v) or math.isinf(v):
            failures.append(f"  {side}.{label}: total_ms が NaN/Inf")
            return None
        return v

    for label, base_v in base_q.items():
        if label not in cur_q:
            failures.append(f"  - 新測定に存在しないクエリ: {label} (bench_search 出力が壊れている疑い)")
            continue
        b = numeric_total_ms(base_v, label, "baseline")
        c = numeric_total_ms(cur_q[label], label, "current")
        if b is None or c is None:
            continue
        if b <= 0.0:
            warnings.append(f"  - {label}: baseline=0ms 以下 (skip)")
            continue
        delta_pct = (c - b) / b * 100.0
        marker = "OK"
        if delta_pct > args.threshold:
            marker = "REGRESSION"
            failures.append(f"  {label}: baseline={b:.1f}ms current={c:.1f}ms (+{delta_pct:.1f}%)")
        elif delta_pct < -args.threshold:
            marker = "FASTER"  # baseline 更新候補
        print(f"{marker:11s}  {label:20s}  baseline={b:7.2f}ms  current={c:7.2f}ms  ({delta_pct:+6.1f}%)")

    for label in cur_q:
        if label not in base_q:
            warnings.append(f"  - baseline に無い新クエリ: {label}")

    if warnings:
        print("\nwarnings:")
        for w in warnings:
            print(w)

    if failures:
        print(f"\n=== 失敗 {len(failures)} 件 (閾値 +{args.threshold}% / 欠落/不正値含む) ===")
        for f in failures:
            print(f)
        return 1

    print("\nOK: 全クエリで回帰なし")
    return 0


if __name__ == "__main__":
    sys.exit(main())
