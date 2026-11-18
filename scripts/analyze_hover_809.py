"""Join §1.809 A/B artifacts without running the application or GPU.

Use representative median pass costs as a model, never as measured GPU utilization.
Present is unmeasured: --present-us is an explicitly assumed equal cost per frame.
"""

import argparse
import json
import math
from pathlib import Path


def estimate(cpu, gpu, present_us=0.0):
    if not math.isfinite(present_us) or present_us < 0:
        raise ValueError("present-us must be finite and nonnegative")
    lookup = {}
    for row in gpu["rows"]:
        key = (row["scene"], tuple(row["size_in_pixels"]), row["pixels_per_point"], row["representative"])
        if key in lookup:
            raise ValueError(f"duplicate GPU row: {key}")
        lookup[key] = row
    rows = []
    for case in cpu["cases"]:
        same = case["identical_frames"]
        changed = case["changed_frames"]
        candidates = case["skip_candidate_frames"]
        assert same + changed == case["frames"] and 0 <= candidates <= same
        prefix = (case["scene"], tuple(case["size_in_pixels"]), case["pixels_per_point"])
        costs = {}
        kinds = set()
        missing = []
        for label, count in [("same", same), ("changed", changed)]:
            row = lookup.get((*prefix, label))
            if count == 0:
                costs[label] = 0.0
            elif row is None:
                missing.append(label)
            else:
                timestamp = row["gpu_render_pass_us"]
                kind = "GPU render-pass timestamp" if timestamp else "CPU submit-completion upper-bound proxy"
                kinds.add(kind)
                costs[label] = (timestamp or row["cpu_submit_to_completion_us"])["median"]
        result = {key: case[key] for key in ["scene", "size_in_pixels", "pixels_per_point", "hz", "moving", "seconds", "frames"]}
        result.update(identical_ratio=case["identical_ratio"], skip_candidate_ratio=case["skip_candidate_ratio"],
                      assumed_equal_present_us=present_us, missing_representatives=missing,
                      timing_kind="; ".join(sorted(kinds)),
                      render_reduction_ratio=None, render_plus_assumed_present_reduction_ratio=None)
        if not missing:
            baseline = same * costs["same"] + changed * costs["changed"]
            saved = candidates * costs["same"]
            total = baseline + case["frames"] * present_us
            if baseline > 0 and total > 0:
                result.update(
                    modeled_render_us_per_second=baseline / case["seconds"],
                    modeled_saved_render_us_per_second=saved / case["seconds"],
                    render_reduction_ratio=saved / baseline,
                    render_plus_assumed_present_reduction_ratio=(saved + candidates * present_us) / total,
                    # With an unknown but equal, nonnegative per-frame present cost,
                    # the combined ratio lies between render-only and candidate fraction.
                    equal_present_sensitivity_ratio_range=[min(saved / baseline, candidates / case["frames"]),
                                                           max(saved / baseline, candidates / case["frames"])],
                )
        rows.append(result)
    return {"section": "1.809", "measurement": "C", "present_measured": False,
            "model": "median representative costs; frames are forced synthetic input, not native FPS; ignores upload/pacing/DWM and callback work",
            "eligibility": "identical primitives AND empty textures_delta AND no callback, fixed single viewport/size/ppp, no screenshot/surface lifecycle",
            "rows": rows}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cpu", type=Path)
    parser.add_argument("gpu", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--present-us", type=float, default=0.0)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    output = args.output.resolve()
    if not output.is_relative_to(root) or not output.parent.is_dir():
        parser.error("output must have an existing parent inside this worktree")
    report = estimate(json.loads(args.cpu.read_text(encoding="utf-8")),
                      json.loads(args.gpu.read_text(encoding="utf-8")), args.present_us)
    with output.open("x", encoding="utf-8") as stream:
        json.dump(report, stream, ensure_ascii=False, indent=2, allow_nan=False)
    print("| scene | pixels / ppp | Hz | moving | identical % | render reduction % | with assumed present % | timing basis |")
    print("| --- | --- | ---: | --- | ---: | ---: | ---: | --- |")
    for row in report["rows"]:
        def percent(field):
            return "unavailable" if row[field] is None else f"{row[field] * 100:.2f}"
        print(f'| {row["scene"]} | {row["size_in_pixels"]} / {row["pixels_per_point"]} | {row["hz"]} | {row["moving"]} '
              f'| {percent("identical_ratio")} | {percent("render_reduction_ratio")} '
              f'| {percent("render_plus_assumed_present_reduction_ratio")} | {row["timing_kind"]} |')


if __name__ == "__main__":
    main()
