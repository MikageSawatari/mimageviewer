#!/usr/bin/env python3
"""bench_search の JSON 出力を baseline と比較し、回帰を検出する。

Usage:
    # 最新計測を取って baseline と比較 (CI 風、デフォルト動作)
    cargo run --release --features dev-tools --bin bench_search -- --docs 50000 --json /tmp/bench_new.json
    python scripts/check_bench_regression.py vendor/bench_baseline.json /tmp/bench_new.json

    # 同じバイナリを 2-3 回測り、query ごとの最小値で判定する (推奨)
    python scripts/check_bench_regression.py vendor/bench_baseline.json run1.json run2.json run3.json

    # 初回登録 / リファレンスの更新 (current は 1 ファイルだけ)
    python scripts/check_bench_regression.py --save --note "dev 機 / 並行ビルドなし" vendor/bench_baseline.json /tmp/bench_new.json

判定:
- query 単位で「baseline 比 +THRESHOLD% 超 (既定 30%)」**かつ**「絶対差が +MIN_ABS_MS ms 超
  (既定 1.0)」の両方を満たしたときだけ回帰扱い (exit 1)。
- 比率だけ超えて絶対差が floor 以下のものは NOISE として表示するが失敗にしない。sub-ms の
  クエリは run 間のばらつきが 30% を軽く超えるので、比率だけでは判定できない (実測:
  super_generic 0.0062 → 0.0176ms = +184%、rare_jp_and は差が 0.08ms)。
- `--min-abs-ms 0` を渡すと floor が無効になり、従来どおり比率だけの判定に戻る。
- current を複数指定した場合は query ごとに **最小の** total_ms を採用する (best-of-N)。
  1 回きりの計測は外れ値を引く (実測: baseline 1.03ms の rare_jp が同一バイナリで
  3.61 / 1.13 / 0.89ms)。floor だけではこの種の外れ値は消えないので、同じバイナリを
  2-3 回測って渡す。
- **ノイズと判断した場合に --save で baseline を上書きしない。** baseline は実測 1 回分の
  値であるべきで、ばらつきを取り込んで上書きすると以後の判定基準がずれる。
- 新たに追加されたクエリは無視 (warning のみ)。baseline にあって全 run に無いクエリは失敗、
  一部の run にだけ無いクエリは warning (残りの run の最小値で判定)。
- `total_ms` が欠落 / 非数値 / NaN / Inf のレコードは失敗 (bench_search 出力が壊れている疑い)。
- `hits` は変化を許容 (corpus 生成シードが固定なので変動は本来無いが、tantivy 側の微妙な
  順位変動で truncated 後の hits 数が動く可能性があり、性能 regression と無関係なため)。
- `--save` 時は baseline に `baseline_meta` (saved_at / 任意の note) を書く。比較は
  `queries` しか見ないので、`baseline_meta` を持たない既存 baseline もそのまま使える。

CLAUDE.md のリリース手順 Phase 2 で実行する。CI が無いプロジェクトなので手動。
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import sys
from pathlib import Path
from typing import NamedTuple

DEFAULT_THRESHOLD_PCT = 30.0
DEFAULT_MIN_ABS_MS = 1.0


class Row(NamedTuple):
    """1 クエリ分の判定結果。"""

    label: str
    marker: str  # OK / FASTER / NOISE / REGRESSION
    baseline_ms: float
    current_ms: float
    delta_pct: float
    delta_ms: float
    runs_used: int


class Comparison(NamedTuple):
    rows: list[Row]
    failures: list[str]
    warnings: list[str]


def load(path: Path) -> dict:
    if not path.exists():
        sys.exit(f"error: {path} が存在しません")
    with path.open("r", encoding="utf-8") as f:
        return json.load(f)


def numeric_total_ms(
    record: object,
    label: str,
    side: str,
    failures: list[str],
) -> float | None:
    """`total_ms` が float として取れない (missing / 非数値 / NaN / Inf) ならエラー扱い。"""
    if not isinstance(record, dict) or "total_ms" not in record:
        failures.append(f"  {side}.{label}: total_ms フィールドが無い")
        return None
    try:
        v = float(record["total_ms"])
    except (TypeError, ValueError):
        failures.append(f"  {side}.{label}: total_ms が数値ではない ({record['total_ms']!r})")
        return None
    # NaN / Inf は JSON 仕様上は無効、Python は読み込めるが比較で罠になる。
    if math.isnan(v) or math.isinf(v):
        failures.append(f"  {side}.{label}: total_ms が NaN/Inf")
        return None
    return v


def compare(
    baseline: dict,
    currents: list[dict],
    threshold_pct: float = DEFAULT_THRESHOLD_PCT,
    min_abs_ms: float = DEFAULT_MIN_ABS_MS,
) -> Comparison:
    """baseline と 1 つ以上の current 計測を突き合わせる純関数 (I/O なし)。

    current が複数ある場合は query ごとに最小の total_ms を採る (best-of-N)。
    参照するのは `queries` だけなので、top-level の `baseline_meta` は自動的に無視される。
    """
    base_q = baseline.get("queries", {})
    cur_qs = [c.get("queries", {}) for c in currents]

    rows: list[Row] = []
    failures: list[str] = []
    warnings: list[str] = []

    for label, base_v in base_q.items():
        present = [(i, q[label]) for i, q in enumerate(cur_qs) if label in q]
        if not present:
            failures.append(
                f"  - 新測定に存在しないクエリ: {label} (bench_search 出力が壊れている疑い)"
            )
            continue
        b = numeric_total_ms(base_v, label, "baseline", failures)
        values: list[float] = []
        for run_index, record in present:
            # レコードが在るのに total_ms が読めないのは「ばらつき」ではなく出力破損なので、
            # 他の run に有効値があっても失敗にする。
            side = "current" if len(cur_qs) == 1 else f"current[run{run_index + 1}]"
            v = numeric_total_ms(record, label, side, failures)
            if v is not None:
                values.append(v)
        if b is None or not values:
            continue
        if len(present) < len(cur_qs):
            warnings.append(
                f"  - {label}: {len(cur_qs)} run 中 {len(present)} run にのみ存在 (最小値で判定)"
            )
        if b <= 0.0:
            warnings.append(f"  - {label}: baseline=0ms 以下 (skip)")
            continue
        c = min(values)
        delta_ms = c - b
        delta_pct = delta_ms / b * 100.0
        marker = "OK"
        if delta_pct > threshold_pct and delta_ms > min_abs_ms:
            marker = "REGRESSION"
            failures.append(
                f"  {label}: baseline={b:.2f}ms current={c:.2f}ms "
                f"(+{delta_pct:.1f}%, +{delta_ms:.2f}ms)"
            )
        elif delta_pct > threshold_pct:
            # 比率だけ超過。絶対差が floor 以下なので計測ノイズとして扱う (失敗にしない)。
            marker = "NOISE"
        elif delta_pct < -threshold_pct:
            marker = "FASTER"  # baseline 更新候補
        rows.append(Row(label, marker, b, c, delta_pct, delta_ms, len(values)))

    seen = set(base_q)
    for q in cur_qs:
        for label in q:
            if label not in seen:
                warnings.append(f"  - baseline に無い新クエリ: {label}")
                seen.add(label)

    return Comparison(rows, failures, warnings)


def baseline_meta(note: str | None) -> dict:
    """--save 時に書く provenance。どの日にどんな条件で取った baseline かを残す。"""
    meta: dict = {"saved_at": dt.date.today().isoformat()}
    if note:
        meta["note"] = note
    return meta


def min_abs_ms_arg(text: str) -> float:
    """--min-abs-ms 用の検証。負値 / NaN / Inf は argparse エラーにする。"""
    try:
        value = float(text)
    except ValueError:
        raise argparse.ArgumentTypeError(f"数値ではありません: {text!r}")
    if math.isnan(value) or math.isinf(value):
        raise argparse.ArgumentTypeError(f"NaN / Inf は指定できません: {text!r}")
    if value < 0.0:
        raise argparse.ArgumentTypeError(f"0 以上を指定してください: {text!r}")
    return value


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("baseline", type=Path, help="基準値の JSON (例: vendor/bench_baseline.json)")
    ap.add_argument(
        "current",
        type=Path,
        nargs="+",
        help="今回計測の JSON (bench_search --json で生成)。複数渡すと query ごとに最小値を採る",
    )
    ap.add_argument(
        "--threshold",
        type=float,
        default=DEFAULT_THRESHOLD_PCT,
        help=f"許容劣化率 (パーセント、既定 {DEFAULT_THRESHOLD_PCT})",
    )
    ap.add_argument(
        "--min-abs-ms",
        type=min_abs_ms_arg,
        default=DEFAULT_MIN_ABS_MS,
        help=(
            f"回帰と見なす最小の絶対差 (ms、既定 {DEFAULT_MIN_ABS_MS})。"
            "比率を超えてもこれ以下の差は NOISE 表示のみ。0 で無効 (比率だけの旧判定)"
        ),
    )
    ap.add_argument(
        "--save",
        action="store_true",
        help="比較せず current を baseline にコピーする (初回登録 / リファレンス更新時)",
    )
    ap.add_argument(
        "--note",
        default=None,
        help="--save 時に baseline_meta.note へ残す自由記述 (例: 機材名 / 並行ビルドの有無)",
    )
    args = ap.parse_args()

    if args.save:
        if len(args.current) != 1:
            # baseline は「実測 1 回分」でなければならない。複数 run の最小値を焼き込むと、
            # どの 1 回の計測でも届かない合成基準になり、以後の誤検知が増える。
            ap.error(
                f"--save には current を 1 ファイルだけ指定してください ({len(args.current)} 個指定されました)。"
                "baseline は実測 1 回分の値であり、best-of-N の合成値は基準に使えません"
            )
        cur = load(args.current[0])
        cur["baseline_meta"] = baseline_meta(args.note)
        args.baseline.parent.mkdir(parents=True, exist_ok=True)
        with args.baseline.open("w", encoding="utf-8") as f:
            json.dump(cur, f, indent=2, ensure_ascii=False)
            f.write("\n")
        print(f"saved baseline: {args.baseline} ({cur['baseline_meta']})")
        return 0

    if args.note is not None:
        ap.error("--note は --save と一緒に使ってください (比較時は保存先がありません)")

    base = load(args.baseline)
    currents = [load(p) for p in args.current]

    if not base.get("queries", {}):
        sys.exit("error: baseline.queries が空 (初回は --save で登録してください)")

    meta = base.get("baseline_meta")
    if isinstance(meta, dict):
        note = meta.get("note")
        suffix = f" note={note}" if note else ""
        print(f"baseline: {args.baseline} (saved_at={meta.get('saved_at', '?')}{suffix})")
    if len(currents) > 1:
        print(f"current {len(currents)} run をマージ (query ごとに最小の total_ms を採用)")
        for i, path in enumerate(args.current, 1):
            print(f"  run{i}: {path}")

    result = compare(base, currents, args.threshold, args.min_abs_ms)

    for row in result.rows:
        print(
            f"{row.marker:11s}  {row.label:20s}  baseline={row.baseline_ms:7.2f}ms  "
            f"current={row.current_ms:7.2f}ms  ({row.delta_pct:+6.1f}%, {row.delta_ms:+7.2f}ms)"
        )

    noise = [row for row in result.rows if row.marker == "NOISE"]
    if noise:
        print(f"\n=== NOISE {len(noise)} 件 (比率は超過、絶対差 +{args.min_abs_ms}ms 以下) ===")
        for row in noise:
            print(f"  {row.label}: +{row.delta_pct:.1f}% / +{row.delta_ms:.3f}ms")
        print("  → ノイズと判断した場合に --save で baseline を上書きしない。")

    if result.warnings:
        print("\nwarnings:")
        for w in result.warnings:
            print(w)

    if result.failures:
        print(
            f"\n=== 失敗 {len(result.failures)} 件 "
            f"(閾値 +{args.threshold}% かつ +{args.min_abs_ms}ms / 欠落・不正値含む) ==="
        )
        for f in result.failures:
            print(f)
        return 1

    print("\nOK: 全クエリで回帰なし")
    return 0


if __name__ == "__main__":
    sys.exit(main())
