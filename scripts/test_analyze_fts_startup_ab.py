import unittest
from pathlib import Path
from unittest.mock import patch

import analyze_fts_startup_ab as ab


class StartupAbTests(unittest.TestCase):
    def manifest(self):
        result = {"schema": 1, "measurement_resolution_ms": 1, "runs": []}
        for name in ("source_revision", "build_profile", "common_build_options", "fixture_preparation", "cache_protocol", "contention_protocol"):
            result[name] = "recorded"
        for condition in ab.CONDITIONS:
            cache, skip, failed, io = condition
            for pair in range(1, 4):
                for arm in ("AB" if pair % 2 else "BA"):
                    result["runs"].append({"cache": cache, "skip_offline_change_scan": skip,
                        "failed_population": failed, "failed_rows": {"zero": 0, "few": 3, "many": 800000}[failed],
                        "io": io, "pair": pair, "arm": arm, "log": "test.jsonl",
                        "fixture_id": "same-normal-fixture", "cache_evidence": "recorded",
                        "io_evidence": "recorded", "binary_sha256": arm})
        return result

    def measured(self, run, _base):
        return ({kind: 100.0 for kind in ab.ENDPOINTS}, {})

    def test_complete_alternating_matrix_equal_timings_ready_for_review(self):
        with patch.object(ab, "read_run", side_effect=self.measured):
            self.assertEqual(ab.analyze(self.manifest(), Path("."))["status"], "evidence_ready_for_review")

    def test_runs_must_be_an_array_and_invalid_manifest_is_not_adopt(self):
        for invalid in (None, False, 1, "runs", {}):
            with self.subTest(runs=invalid):
                manifest = self.manifest()
                manifest["runs"] = invalid
                report = ab.analyze(manifest, Path("."))
                self.assertEqual(report["status"], "not_adopt")
                self.assertIn("manifest runs must be an array", report["reasons"])

    def test_any_positive_delta_is_not_adopt_even_below_resolution(self):
        def measured(run, base):
            values, diagnostic = self.measured(run, base)
            if run["arm"] == "B":
                values[ab.ENDPOINTS[0]] += .00001
            return values, diagnostic
        with patch.object(ab, "read_run", side_effect=measured):
            report = ab.analyze(self.manifest(), Path("."))
        self.assertEqual(report["status"], "not_adopt")
        self.assertTrue(any("regression" in reason for reason in report["reasons"]))

    def test_mixed_noise_is_not_adopt(self):
        def measured(run, base):
            values, diagnostic = self.measured(run, base)
            if run["arm"] == "B":
                values[ab.ENDPOINTS[0]] += 1 if run["pair"] == 1 else -1
            return values, diagnostic
        with patch.object(ab, "read_run", side_effect=measured):
            report = ab.analyze(self.manifest(), Path("."))
        self.assertTrue(any("noisy_or_inconclusive" in reason for reason in report["reasons"]))

    def test_missing_condition_and_non_alternating_order_are_not_adopt(self):
        manifest = self.manifest()
        del manifest["runs"][-6:]
        manifest["runs"][2:4] = list(reversed(manifest["runs"][2:4]))
        with patch.object(ab, "read_run", side_effect=self.measured):
            report = ab.analyze(manifest, Path("."))
        self.assertEqual(report["status"], "not_adopt")
        self.assertTrue(any("fewer than 3" in reason for reason in report["reasons"]))
        self.assertTrue(any("alternating" in reason for reason in report["reasons"]))

    def events(self, enabled=True):
        result = [{"cat": "startup", "kind": kind, "t": .1, "ready": True, "accepted": True,
                   "integrity_feature_enabled": enabled} for kind in ab.ENDPOINTS]
        result.append({"cat": "startup", "kind": "metadata_out_of_range_cleanup", "t": .2,
                       "integrity_feature_enabled": enabled, "success": True, "rows": 0, "owner_rows": 840464})
        if enabled:
            result.append({"cat": "startup", "kind": "fts_integrity_check", "t": .15,
                "feature_enabled": True, "owner_rows": 840464, "not_ok_rows": 0, "ok_rows": 840464,
                "segments": 5, "live_docs": 840464, "integrity_check_ms": .02, "missing_suspected": False})
        return result

    def read(self, events, delivery="complete"):
        run = self.manifest()["runs"][1]
        with patch.object(ab, "load_events", return_value=events), patch.object(ab, "validate_delivery", return_value={"status": delivery, "reasons": ["missing checkpoint"]}):
            return ab.read_run(run, Path("."))

    def test_normal_live_counts_and_feature_are_required(self):
        self.read(self.events())
        for field, value in (("missing_suspected", True), ("live_docs", 840465), ("not_ok_rows", 1), ("feature_enabled", False)):
            events = self.events()
            events[-1][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.read(events)

    def test_missing_endpoint_and_dropped_or_unflushed_log_are_not_evidence(self):
        with self.assertRaises(ValueError):
            self.read(self.events()[1:])
        with self.assertRaises(ValueError):
            self.read(self.events(), delivery="CannotJudge")

    def test_a_must_not_run_check(self):
        run = self.manifest()["runs"][0]
        with patch.object(ab, "load_events", return_value=self.events(False)), patch.object(ab, "validate_delivery", return_value={"status": "complete"}):
            ab.read_run(run, Path("."))
        events = self.events(False)
        events.append(self.events()[-1])
        with patch.object(ab, "load_events", return_value=events), self.assertRaises(ValueError):
            ab.read_run(run, Path("."))

    def test_shutdown_coverage_and_drop_evidence_are_checked(self):
        from test_analyze_perf import delivery_events
        run = self.manifest()["runs"][1]
        events = delivery_events(self.events(), end_t=.2)
        with patch.object(ab, "load_events", return_value=events):
            ab.read_run(run, Path("."))
            events[-1]["perf_dropped_total"] = 1
            with self.assertRaises(ValueError):
                ab.read_run(run, Path("."))


if __name__ == "__main__":
    unittest.main()
