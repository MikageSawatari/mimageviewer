"""Compare private JSONL locally; print aggregate counts only (standard library)."""
import argparse
import json
import pathlib
import statistics


def read(path):
    rows = [json.loads(line) for line in pathlib.Path(path).read_text(encoding="utf-8").splitlines() if line.strip()]
    by_image = {r["image"]: r for r in rows}
    if len(by_image) != len(rows):
        raise ValueError("Duplicate images")
    return rows, by_image


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--baseline", required=True)
    ap.add_argument("--forward", required=True)
    ap.add_argument("--reverse", required=True)
    ap.add_argument("--reference", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    _, baseline = read(a.baseline)
    rows, forward = read(a.forward)
    reverse_rows, reverse = read(a.reverse)
    _, reference = read(a.reference)
    if not (baseline.keys() == forward.keys() == reverse.keys() == reference.keys()):
        raise ValueError("Image sets differ")
    fields = ("caption", "text", "keywords")
    result = {"count": len(rows)}
    for label, other in (("C9", baseline), ("order_reverse", reverse)):
        result[label] = {
            "all_three_equal": sum(all(r.get(f) == other[k].get(f) for f in fields) for k, r in forward.items()),
            "field_equal": {f: sum(r.get(f) == other[k].get(f) for k, r in forward.items()) for f in fields},
            "finish_reason_equal": sum(r.get("finish_reason") == other[k].get("finish_reason") for k, r in forward.items()),
            "usage_equal": sum(r.get("usage") == other[k].get("usage") for k, r in forward.items()),
        }
    result["reference_chunks_equal"] = sum(r["prompt_chunks"] == reference[k]["prompt_chunks"] for k, r in forward.items())
    result["C9_prompt_token_count_equal"] = sum(r["usage"]["prompt_tokens"] == baseline[k]["usage"]["prompt_tokens"] for k, r in forward.items())
    for label, rs in (("forward", rows), ("reverse", reverse_rows)):
        result[label] = {
            "validation_ok": sum(r.get("validation", {}).get("ok", False) for r in rs),
            "validation_checks": {f: sum(r.get("validation", {}).get(f, False) for r in rs) for f in ("ended_on_eog", "json_complete", "field_types", "limits", "token_limit")},
            "median_excluding_first_s": statistics.median(r["wall_s"] for r in rs[1:]),
            "first_s": rs[0]["wall_s"],
            "model_load_s": rs[0]["model_load_s"],
        }
    pathlib.Path(a.out).write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
