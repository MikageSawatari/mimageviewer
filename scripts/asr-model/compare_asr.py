"""ASR worker の出力を評価の正本と照合する。本文は標準出力に出さない。"""
import argparse
import hashlib
import json
from pathlib import Path


def normalized(segments):
    return [(round(s["start"] * 1000), round(s["end"] * 1000), s["text"])
            for s in segments]


def digest(segments):
    return hashlib.sha256(json.dumps(segments, ensure_ascii=False,
                                    separators=(",", ":")).encode("utf-8")).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", type=Path, required=True)
    parser.add_argument("--evaluation-work", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    report = json.loads(args.run.read_text(encoding="utf-8"))
    results = []
    for file in report["files"]:
        source = Path(file["input"])
        name = source.parent.name
        if name in ("m1_explainer", "m2_conversation", "m3_fleurs_clean", "m4_fleurs_noisy", "m5_video"):
            reference_path = args.evaluation_work / "runs/turbo-q5_0-vulkan-mc0" / name / "transcript.json"
        elif name in ("m6", "m6_long"):
            name = "m6"
            reference_path = args.evaluation_work / "runs/t0/m6/follow/transcript.json"
        elif source.stem == "G":
            name = "G"
            reference_path = args.evaluation_work / "runs/t0/uv-G/follow/transcript.json"
        else:
            raise ValueError(f"Unknown evaluation input: {source}")
        reference = json.loads(reference_path.read_text(encoding="utf-8"))
        expected, actual = normalized(reference["segments"]), normalized(file["segments"])
        result = {"material": name, "segments": len(actual), "reference_segments": len(expected),
                  "exact_match": expected == actual, "actual_sha256": digest(actual),
                  "reference_sha256": digest(expected),
                  "different_positions": sum(a != b for a, b in zip(actual, expected)) + abs(len(actual) - len(expected)),
                  "windows": file["windows"], "wall_s": file["wall_s"]}
        results.append(result)
        print(f"{name}: segments={len(actual)}/{len(expected)} exact={result['exact_match']} windows={len(file['windows'])}")
    with args.out.open("x", encoding="utf-8") as output:
        json.dump({"results": results, "process_peaks": report["process_peaks"]}, output, ensure_ascii=False, indent=2)
        output.write("\n")
    # 差があればコードではなく検収の判断に返す。
    return 0 if all(r["exact_match"] for r in results) else 2


if __name__ == "__main__":
    raise SystemExit(main())
