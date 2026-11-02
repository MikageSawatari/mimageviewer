"""Compare two private selftest runs; emit aggregate results only."""
import argparse
import json
import pathlib


def read(path):
    rows = [json.loads(line) for line in pathlib.Path(path).read_text(encoding="utf-8").splitlines() if line.strip()]
    result = {row["image"]: row for row in rows}
    if len(result) != len(rows):
        raise ValueError("Duplicate images")
    return result


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--previous", required=True)
    ap.add_argument("--current", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    previous, current = read(a.previous), read(a.current)
    if not current or current.keys() != previous.keys():
        raise ValueError("Image sets differ or are empty")
    fields = ("caption", "text", "keywords", "content", "finish_reason", "usage", "prompt_chunks")
    equal = {field: sum(row.get(field) == previous[name].get(field) for name, row in current.items()) for field in fields}
    result = {"count": len(current), "validation_ok": sum(row["validation"]["ok"] for row in current.values()),
              "field_equal": equal, "max_keyword_chars": max(len(word) for row in current.values() for word in row.get("keywords", []))}
    pathlib.Path(a.out).write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2))
    if result["validation_ok"] != len(current) or any(n != len(current) for n in equal.values()):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
