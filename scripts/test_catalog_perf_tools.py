"""Headless catalog instrumentation and benchmark evidence regressions."""
import contextlib
import io
import unittest

from analyze_catalog_bench import summarize
from analyze_perf import cmd_nav


class CatalogPerfToolsTests(unittest.TestCase):
    def test_nav_preserves_old_and_new_catalog_intervals_separately(self):
        events = [
            {"cat": "nav", "kind": "sli_catalog_load_all", "ms": 150},
            {"cat": "nav", "kind": "sli_catalog_load_all", "ms": 170},
            {"cat": "nav", "kind": "sli_catalog_prepare", "ms": 0.2},
        ]
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            cmd_nav(events)
        lines = out.getvalue().splitlines()
        legacy = next(line for line in lines if line.strip().startswith("sli_catalog_load_all"))
        sparse = next(line for line in lines if line.strip().startswith("sli_catalog_prepare"))
        self.assertIn("n=2", legacy)
        self.assertIn("150.0", legacy)
        self.assertIn("170.0", legacy)
        self.assertIn("n=1", sparse)
        self.assertIn("0.2", sparse)
        self.assertNotIn("150.0", sparse)

    def test_nav_shows_sparse_preparation_seed_reads_and_writeback_outcomes(self):
        events = [
            {"cat": "nav", "kind": "sli_catalog_prepare", "ms": 0.2, "entries": 0},
            {"cat": "nav", "kind": "sli_virtual_seed", "ms": 1, "catalog_reads": 1},
            {"cat": "nav", "kind": "sli_video_pin_seed", "ms": 3, "catalog_reads": 200},
            {"cat": "thumb", "kind": "virtual_writeback", "ms": 2, "copied": True},
            {"cat": "thumb", "kind": "virtual_writeback", "ms": 1, "copied": False},
        ]
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            cmd_nav(events)
        rendered = out.getvalue()
        self.assertIn("sli_catalog_prepare", rendered)
        self.assertIn("sli_video_pin_seed: events=1 measured=1 reads=200", rendered)
        self.assertIn("copied=1 skipped=1 unknown=0", rendered)

    def complete_rows(self):
        rows = [{"kind": "protocol", "version": 1, "repetitions": 3, "worker_counts": [1, 4]}]
        for shape in ("full", "empty", "stale90", "partial50"):
            for connection in ("fresh", "warm"):
                for workers in (1, 4):
                    for mode in ("memory", "db", "source"):
                        for repetition in range(3):
                            rows.append({"kind": "catalog", "shape": shape, "connection": connection,
                                         "workers": workers, "mode": mode, "repetition": repetition,
                                         "ui_prepare_ms": 100 if mode == "memory" else 1,
                                         "initial_ready_ms": 110 if mode == "memory" else 11,
                                         "initial": {"all_visible_ms": 10}, "scroll": {"all_visible_ms": 10}})
        for background in (0, 10_000):
            for shape in ("empty", "same", "partial"):
                for connection in ("fresh", "warm"):
                    for mode in ("memory", "db"):
                        for repetition in range(3):
                            rows.append({"kind": "seed", "background_rows": background, "shape": shape,
                                         "connection": connection, "mode": mode, "repetition": repetition,
                                         "ui_prepare_ms": 20 if mode == "memory" else 10, "seed_ms": 10})
        return rows

    def test_complete_evidence_and_no_regression_pass(self):
        self.assertTrue(summarize(self.complete_rows())["pass"])

    def test_truncation_and_missing_connection_cannot_pass(self):
        rows = self.complete_rows()
        self.assertFalse(summarize(rows[:-1])["pass"])
        self.assertFalse(summarize([row for row in rows if row.get("connection") != "fresh"])["pass"])
        self.assertFalse(summarize([row for row in rows if row.get("workers") != 4])["pass"])

    def test_seed_overhead_on_empty_db_is_not_hidden_by_background_saving(self):
        rows = self.complete_rows()
        for row in rows:
            if row.get("kind") == "seed" and row["background_rows"] == 0 and row["shape"] == "empty" and row["mode"] == "db":
                row["ui_prepare_ms"] = 20.5
        strict = summarize(rows)
        self.assertFalse(strict["gates"]["3_200_pin_ui_preparation_no_slower"])
        self.assertTrue(summarize(rows, noise_ms=1)["pass"])


if __name__ == "__main__":
    unittest.main()
