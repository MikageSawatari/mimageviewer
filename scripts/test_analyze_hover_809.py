"""Small arithmetic/eligibility tests; no product code or GPU executed."""
import unittest

from analyze_hover_809 import estimate


class Estimates(unittest.TestCase):
    def fixture(self):
        case = dict(scene="s", size_in_pixels=[2560, 1440], pixels_per_point=1.0,
                    hz=60, moving=True, seconds=1, frames=60, identical_frames=48,
                    changed_frames=12, skip_candidate_frames=42, identical_ratio=0.8,
                    skip_candidate_ratio=0.7)
        rows = [dict(scene="s", size_in_pixels=[2560, 1440], pixels_per_point=1.0,
                     representative=label, gpu_render_pass_us=dict(median=cost))
                for label, cost in [("same", 10), ("changed", 20)]]
        return dict(cases=[case]), dict(rows=rows)

    def test_weighted_cost_excludes_same_frames_with_texture_work(self):
        cpu, gpu = self.fixture()
        row = estimate(cpu, gpu, 5)["rows"][0]
        self.assertAlmostEqual(row["render_reduction_ratio"], 420 / 720)
        self.assertAlmostEqual(row["render_plus_assumed_present_reduction_ratio"], 630 / 1020)
        self.assertEqual(row["equal_present_sensitivity_ratio_range"], [420 / 720, 0.7])

    def test_missing_class_is_unavailable(self):
        cpu, gpu = self.fixture()
        gpu["rows"].pop()
        row = estimate(cpu, gpu)["rows"][0]
        self.assertIsNone(row["render_reduction_ratio"])
        self.assertEqual(row["missing_representatives"], ["changed"])

    def test_zero_count_class_does_not_need_a_representative(self):
        cpu, gpu = self.fixture()
        cpu["cases"][0].update(identical_frames=60, changed_frames=0)
        gpu["rows"].pop()
        self.assertAlmostEqual(estimate(cpu, gpu)["rows"][0]["render_reduction_ratio"], 0.7)

    def test_fallback_stays_labeled_as_cpu_proxy(self):
        cpu, gpu = self.fixture()
        for row in gpu["rows"]:
            row["cpu_submit_to_completion_us"] = row.pop("gpu_render_pass_us")
            row["gpu_render_pass_us"] = None
        self.assertIn("CPU", estimate(cpu, gpu)["rows"][0]["timing_kind"])


if __name__ == "__main__":
    unittest.main()
