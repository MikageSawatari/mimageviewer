#!/usr/bin/env python3
"""check_bench_regression.py の標準ライブラリだけで動く回帰テスト。

実測値をそのまま使う: vendor/bench_baseline.json の baseline に対し、同一バイナリを
3 回測ったときの rare_jp が 3.61 / 1.13 / 0.89ms、super_generic が +184%、
rare_jp_and の差が 0.08ms だった (backlog §5.8)。製品側の回帰は無かった。
"""

from __future__ import annotations

import contextlib
import datetime as dt
import io
import json
import shutil
import sys
import unittest
import uuid
from pathlib import Path
from unittest import mock

from check_bench_regression import (
    DEFAULT_MIN_ABS_MS,
    DEFAULT_THRESHOLD_PCT,
    baseline_meta,
    compare,
    main,
)

REPO_ROOT = Path(__file__).resolve().parent.parent


@contextlib.contextmanager
def writable_test_directory():
    """Avoid Python 3.13's owner-only Windows temp ACL in restricted runners."""
    base = REPO_ROOT / "target" / "test-check-bench-regression"
    base.mkdir(parents=True, exist_ok=True)
    path = base / f"run-{uuid.uuid4().hex}"
    path.mkdir()
    try:
        yield path
    finally:
        shutil.rmtree(path, ignore_errors=True)


def run_json(queries: dict[str, object]) -> dict:
    """bench_search の JSON 形。値は total_ms (数値) か record そのもの。"""
    records: dict[str, object] = {}
    for label, value in queries.items():
        if isinstance(value, dict):
            records[label] = value
        else:
            records[label] = {"hits": 1, "post_ms": 0.0, "total_ms": value}
    return {"num_docs": 50000, "queries": records, "version": 1}


# vendor/bench_baseline.json の実値 (sub-ms 3 本 / 1-5ms 1 本 / 5ms 超 1 本)。
BASELINE = run_json(
    {
        "generic_jp": 25.0732,
        "medium_jp": 7.2017,
        "medium_jp_3": 0.7442,
        "rare_jp": 1.034,
        "rare_jp_and": 0.15430000000000002,
        "super_generic": 0.0062,
    }
)


def markers(result) -> dict[str, str]:
    return {row.label: row.marker for row in result.rows}


def by_label(result) -> dict[str, object]:
    return {row.label: row for row in result.rows}


class AbsoluteFloorTests(unittest.TestCase):
    def test_sub_millisecond_ratio_excess_is_noise_not_a_failure(self) -> None:
        current = run_json(
            {
                "generic_jp": 25.0732,
                "medium_jp": 7.2017,
                "medium_jp_3": 0.7442,
                "rare_jp": 1.034,
                "rare_jp_and": 0.2343,  # +0.08ms
                "super_generic": 0.0176,  # +184%
            }
        )

        result = compare(BASELINE, [current])

        self.assertEqual(result.failures, [])
        self.assertEqual(markers(result)["super_generic"], "NOISE")
        self.assertEqual(markers(result)["rare_jp_and"], "NOISE")
        # 比率だけ見れば閾値を大きく超えている = floor が無ければ両方とも失敗していた。
        rows = by_label(result)
        self.assertGreater(rows["super_generic"].delta_pct, 180.0)
        self.assertGreater(rows["rare_jp_and"].delta_pct, DEFAULT_THRESHOLD_PCT)
        self.assertLess(rows["rare_jp_and"].delta_ms, DEFAULT_MIN_ABS_MS)

    def test_ratio_and_floor_both_exceeded_is_a_regression(self) -> None:
        current = run_json(
            {
                "generic_jp": 40.0,  # +59.5% / +14.93ms
                "medium_jp": 7.2017,
                "medium_jp_3": 2.5,  # +236% / +1.76ms (sub-ms でも floor を超える)
                "rare_jp": 1.034,
                "rare_jp_and": 0.15430000000000002,
                "super_generic": 0.0062,
            }
        )

        result = compare(BASELINE, [current])

        self.assertEqual(markers(result)["generic_jp"], "REGRESSION")
        self.assertEqual(markers(result)["medium_jp_3"], "REGRESSION")
        self.assertEqual(len(result.failures), 2)
        self.assertTrue(any("generic_jp" in f for f in result.failures))
        self.assertTrue(any("medium_jp_3" in f for f in result.failures))

    def test_min_abs_ms_zero_restores_the_ratio_only_gate(self) -> None:
        current = run_json(
            {
                "generic_jp": 25.0732,
                "medium_jp": 7.2017,
                "medium_jp_3": 0.7442,
                "rare_jp": 1.034,
                "rare_jp_and": 0.2343,
                "super_generic": 0.0176,
            }
        )

        result = compare(BASELINE, [current], min_abs_ms=0.0)

        self.assertEqual(markers(result)["super_generic"], "REGRESSION")
        self.assertEqual(markers(result)["rare_jp_and"], "REGRESSION")
        self.assertEqual(len(result.failures), 2)
        self.assertNotIn("NOISE", set(markers(result).values()))

    def test_faster_and_ok_markers_are_unchanged(self) -> None:
        current = run_json(
            {
                "generic_jp": 10.0,  # -60%
                "medium_jp": 7.3,  # +1.4%
                "medium_jp_3": 0.7442,
                "rare_jp": 1.034,
                "rare_jp_and": 0.15430000000000002,
                "super_generic": 0.0062,
            }
        )

        result = compare(BASELINE, [current])

        self.assertEqual(markers(result)["generic_jp"], "FASTER")
        self.assertEqual(markers(result)["medium_jp"], "OK")
        self.assertEqual(result.failures, [])

    def test_baseline_at_or_below_zero_warns_and_skips(self) -> None:
        baseline = run_json({"zero_ms": 0.0})
        current = run_json({"zero_ms": 5.0})

        result = compare(baseline, [current])

        self.assertEqual(result.rows, [])
        self.assertEqual(result.failures, [])
        self.assertTrue(any("baseline=0ms 以下" in w for w in result.warnings))


class BestOfNTests(unittest.TestCase):
    """同一バイナリの 3 回計測 (rare_jp = 3.61 / 1.13 / 0.89ms) を最小値で判定する。"""

    def test_best_of_three_takes_the_minimum_per_query(self) -> None:
        runs = [
            run_json({"rare_jp": 3.61, "medium_jp": 9.0}),
            run_json({"rare_jp": 1.13, "medium_jp": 7.3}),
            run_json({"rare_jp": 0.89, "medium_jp": 7.1}),
        ]
        baseline = run_json({"rare_jp": 1.034, "medium_jp": 7.2017})

        result = compare(baseline, runs)

        rows = by_label(result)
        self.assertAlmostEqual(rows["rare_jp"].current_ms, 0.89)
        self.assertAlmostEqual(rows["medium_jp"].current_ms, 7.1)
        self.assertEqual(rows["rare_jp"].runs_used, 3)
        self.assertEqual(result.failures, [])

    def test_the_floor_alone_does_not_absorb_the_rare_jp_outlier(self) -> None:
        # 3.61ms は baseline 比 +2.58ms なので floor を超える。best-of-N が必要な理由。
        baseline = run_json({"rare_jp": 1.034})

        single = compare(baseline, [run_json({"rare_jp": 3.61})])
        self.assertEqual(markers(single)["rare_jp"], "REGRESSION")

        merged = compare(
            baseline,
            [
                run_json({"rare_jp": 3.61}),
                run_json({"rare_jp": 1.13}),
                run_json({"rare_jp": 0.89}),
            ],
        )
        self.assertEqual(markers(merged)["rare_jp"], "OK")

    def test_single_current_file_behaves_as_before(self) -> None:
        baseline = run_json({"rare_jp": 1.034})
        result = compare(baseline, [run_json({"rare_jp": 1.13})])

        self.assertEqual(markers(result)["rare_jp"], "OK")
        self.assertEqual(by_label(result)["rare_jp"].runs_used, 1)
        self.assertEqual(result.warnings, [])

    def test_query_missing_from_one_run_uses_the_other_runs(self) -> None:
        baseline = run_json({"rare_jp": 1.034, "medium_jp": 7.2017})
        runs = [
            run_json({"medium_jp": 7.1}),
            run_json({"rare_jp": 0.89, "medium_jp": 7.3}),
        ]

        result = compare(baseline, runs)

        rows = by_label(result)
        self.assertAlmostEqual(rows["rare_jp"].current_ms, 0.89)
        self.assertEqual(rows["rare_jp"].runs_used, 1)
        self.assertEqual(result.failures, [])
        self.assertTrue(any("2 run 中 1 run にのみ存在" in w for w in result.warnings))

    def test_query_missing_from_all_runs_fails(self) -> None:
        baseline = run_json({"rare_jp": 1.034})
        runs = [run_json({"medium_jp": 7.1}), run_json({"medium_jp": 7.3})]

        result = compare(baseline, runs)

        self.assertEqual(len(result.failures), 1)
        self.assertIn("新測定に存在しないクエリ: rare_jp", result.failures[0])

    def test_new_query_is_a_single_warning_across_runs(self) -> None:
        baseline = run_json({"rare_jp": 1.034})
        runs = [
            run_json({"rare_jp": 1.0, "brand_new": 2.0}),
            run_json({"rare_jp": 1.0, "brand_new": 2.0}),
        ]

        result = compare(baseline, runs)

        self.assertEqual(result.failures, [])
        self.assertEqual(
            [w for w in result.warnings if "brand_new" in w],
            ["  - baseline に無い新クエリ: brand_new"],
        )


class InvalidValueTests(unittest.TestCase):
    def test_invalid_total_ms_is_a_failure(self) -> None:
        baseline = run_json({"rare_jp": 1.034})
        cases = {
            "missing": {"hits": 1, "post_ms": 0.0},
            "non_numeric": {"total_ms": "abc"},
            "nan": {"total_ms": float("nan")},
            "inf": {"total_ms": float("inf")},
            "not_a_record": 42,
        }

        for case, record in cases.items():
            with self.subTest(case=case):
                # record をそのまま queries に置く (run_json は数値を total_ms に包むため)。
                current = {"num_docs": 50000, "queries": {"rare_jp": record}, "version": 1}
                result = compare(baseline, [current])
                self.assertEqual(result.rows, [])
                self.assertEqual(len(result.failures), 1, result.failures)
                self.assertIn("current.rare_jp", result.failures[0])

    def test_invalid_value_in_one_run_fails_even_if_another_run_is_valid(self) -> None:
        # レコードが在るのに total_ms が読めないのは bench 出力の破損。ばらつきではない。
        baseline = run_json({"rare_jp": 1.034})
        runs = [run_json({"rare_jp": {"total_ms": float("nan")}}), run_json({"rare_jp": 0.89})]

        result = compare(baseline, runs)

        self.assertEqual(len(result.failures), 1, result.failures)
        self.assertIn("current[run1].rare_jp", result.failures[0])

    def test_invalid_baseline_value_is_a_failure(self) -> None:
        baseline = run_json({"rare_jp": {"total_ms": "x"}})

        result = compare(baseline, [run_json({"rare_jp": 1.0})])

        self.assertEqual(result.rows, [])
        self.assertTrue(any("baseline.rare_jp" in f for f in result.failures))


class BaselineMetaTests(unittest.TestCase):
    def test_baseline_meta_is_ignored_when_comparing(self) -> None:
        with_meta = dict(BASELINE)
        with_meta["baseline_meta"] = {"saved_at": "2026-09-17", "note": "dev 機"}

        result = compare(with_meta, [run_json({"rare_jp": 1.0})])

        self.assertNotIn("baseline_meta", markers(result))
        self.assertFalse(any("baseline_meta" in w for w in result.warnings))

    def test_meta_helper_writes_todays_date_and_optional_note(self) -> None:
        today = dt.date.today().isoformat()

        self.assertEqual(baseline_meta(None), {"saved_at": today})
        self.assertEqual(baseline_meta(""), {"saved_at": today})
        self.assertEqual(
            baseline_meta("dev 機 / 並行ビルドなし"),
            {"saved_at": today, "note": "dev 機 / 並行ビルドなし"},
        )


def invoke(argv: list[str]) -> tuple[int, str, str]:
    """main() を argv 差し替えで呼ぶ。argparse の SystemExit も exit code に畳む。"""
    stdout = io.StringIO()
    stderr = io.StringIO()
    code = 0
    with (
        mock.patch.object(sys, "argv", ["check_bench_regression.py", *argv]),
        contextlib.redirect_stdout(stdout),
        contextlib.redirect_stderr(stderr),
    ):
        try:
            code = main()
        except SystemExit as error:
            code = int(error.code or 0)
    return code, stdout.getvalue(), stderr.getvalue()


def write_json(path: Path, payload: dict) -> Path:
    path.write_text(json.dumps(payload, ensure_ascii=False), encoding="utf-8")
    return path


class CliTests(unittest.TestCase):
    def test_noise_exits_zero_and_regression_exits_one(self) -> None:
        with writable_test_directory() as temp_dir:
            base = write_json(temp_dir / "baseline.json", BASELINE)
            noisy = write_json(
                temp_dir / "noisy.json",
                run_json(
                    {
                        "generic_jp": 25.0732,
                        "medium_jp": 7.2017,
                        "medium_jp_3": 0.7442,
                        "rare_jp": 1.034,
                        "rare_jp_and": 0.2343,
                        "super_generic": 0.0176,
                    }
                ),
            )
            slow = write_json(
                temp_dir / "slow.json",
                run_json(
                    {
                        "generic_jp": 40.0,
                        "medium_jp": 7.2017,
                        "medium_jp_3": 0.7442,
                        "rare_jp": 1.034,
                        "rare_jp_and": 0.15430000000000002,
                        "super_generic": 0.0062,
                    }
                ),
            )

            code, out, _ = invoke([str(base), str(noisy)])
            self.assertEqual(code, 0, out)
            self.assertIn("NOISE", out)
            self.assertIn("--save で baseline を上書きしない", out)

            code, out, _ = invoke([str(base), str(noisy), "--min-abs-ms", "0"])
            self.assertEqual(code, 1, out)

            code, out, _ = invoke([str(base), str(slow)])
            self.assertEqual(code, 1, out)
            self.assertIn("REGRESSION", out)

    def test_three_runs_are_merged_and_reported(self) -> None:
        with writable_test_directory() as temp_dir:
            base = write_json(temp_dir / "baseline.json", run_json({"rare_jp": 1.034}))
            paths = [
                write_json(temp_dir / f"run{i}.json", run_json({"rare_jp": ms}))
                for i, ms in enumerate([3.61, 1.13, 0.89], 1)
            ]

            code, out, _ = invoke([str(base), *[str(p) for p in paths]])

            self.assertEqual(code, 0, out)
            self.assertIn("current 3 run をマージ", out)
            self.assertIn("OK: 全クエリで回帰なし", out)

    def test_save_writes_baseline_meta_and_keeps_queries(self) -> None:
        with writable_test_directory() as temp_dir:
            measured = write_json(temp_dir / "measured.json", BASELINE)
            saved = temp_dir / "new" / "baseline.json"

            code, out, _ = invoke(
                ["--save", "--note", "dev 機 / 並行ビルドなし", str(saved), str(measured)]
            )

            self.assertEqual(code, 0, out)
            written = json.loads(saved.read_text(encoding="utf-8"))
            self.assertEqual(
                written["baseline_meta"],
                {"saved_at": dt.date.today().isoformat(), "note": "dev 機 / 並行ビルドなし"},
            )
            self.assertEqual(written["queries"], BASELINE["queries"])

            code, out, _ = invoke(["--save", str(saved), str(measured)])
            self.assertEqual(code, 0, out)
            written = json.loads(saved.read_text(encoding="utf-8"))
            self.assertEqual(written["baseline_meta"], {"saved_at": dt.date.today().isoformat()})

            # provenance 付き baseline も従来どおり比較できる。
            code, out, _ = invoke([str(saved), str(measured)])
            self.assertEqual(code, 0, out)
            self.assertIn("saved_at=", out)

    def test_save_rejects_multiple_current_files(self) -> None:
        with writable_test_directory() as temp_dir:
            first = write_json(temp_dir / "a.json", BASELINE)
            second = write_json(temp_dir / "b.json", BASELINE)
            saved = temp_dir / "baseline.json"

            code, _, err = invoke(["--save", str(saved), str(first), str(second)])

            self.assertEqual(code, 2, err)
            self.assertIn("--save には current を 1 ファイルだけ", err)
            self.assertFalse(saved.exists())

    def test_note_without_save_is_rejected(self) -> None:
        with writable_test_directory() as temp_dir:
            base = write_json(temp_dir / "baseline.json", BASELINE)

            code, _, err = invoke([str(base), str(base), "--note", "x"])

            self.assertEqual(code, 2, err)
            self.assertIn("--note は --save と一緒に", err)

    def test_min_abs_ms_rejects_negative_and_nan(self) -> None:
        with writable_test_directory() as temp_dir:
            base = write_json(temp_dir / "baseline.json", BASELINE)
            for bad in ("-1", "nan", "inf", "abc"):
                with self.subTest(value=bad):
                    code, _, err = invoke([str(base), str(base), "--min-abs-ms", bad])
                    self.assertEqual(code, 2, err)
                    self.assertIn("--min-abs-ms", err)

    def test_repository_baseline_compares_against_itself(self) -> None:
        # baseline_meta を持たない実ファイルがそのまま通ることの確認。
        repo_baseline = REPO_ROOT / "vendor" / "bench_baseline.json"
        if not repo_baseline.is_file():
            self.skipTest("vendor/bench_baseline.json が無い")

        code, out, _ = invoke([str(repo_baseline), str(repo_baseline)])

        self.assertEqual(code, 0, out)
        self.assertIn("OK: 全クエリで回帰なし", out)
        self.assertNotIn("NOISE", out)


if __name__ == "__main__":
    unittest.main()
