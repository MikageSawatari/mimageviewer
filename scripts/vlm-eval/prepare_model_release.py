"""Prepare the local AI caption model release assets (vlm-model-v1).

Splits the quantized model into parts under 2 GiB, copies the vision projector,
LICENSE and NOTICE, and writes vlm-model-index.json (schema 1) as read by
src/vlm_addon.rs. Nothing is uploaded; the output folder is what a person
attaches to the GitHub Release.

Usage:
  python scripts/vlm-eval/prepare_model_release.py --model <gguf> --mmproj <gguf>
      --license <LICENSE> --notice <NOTICE> --out <empty folder>
"""

import argparse
import hashlib
import json
import os
import sys

MAX_PART = 2 * 1024 * 1024 * 1024 - 1
CHUNK = 8 * 1024 * 1024


def sha256_of(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while True:
            b = f.read(CHUNK)
            if not b:
                break
            h.update(b)
    return h.hexdigest()


def copy_hashed(src, dst):
    h = hashlib.sha256()
    n = 0
    with open(src, "rb") as fi, open(dst, "xb") as fo:
        while True:
            b = fi.read(CHUNK)
            if not b:
                break
            h.update(b)
            fo.write(b)
            n += len(b)
    return n, h.hexdigest()


def split(src, out, part_bytes):
    name = os.path.basename(src)
    parts = []
    whole = hashlib.sha256()
    total = 0
    with open(src, "rb") as fi:
        index = 1
        while True:
            part_name = f"{name}.part{index}"
            ph = hashlib.sha256()
            written = 0
            with open(os.path.join(out, part_name), "xb") as fo:
                while written < part_bytes:
                    b = fi.read(min(CHUNK, part_bytes - written))
                    if not b:
                        break
                    ph.update(b)
                    whole.update(b)
                    fo.write(b)
                    written += len(b)
            if written == 0:
                os.remove(os.path.join(out, part_name))
                break
            parts.append({"name": part_name, "bytes": written, "sha256": ph.hexdigest()})
            total += written
            index += 1
            if written < part_bytes:
                break
    return total, whole.hexdigest(), parts


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True)
    ap.add_argument("--mmproj", required=True)
    ap.add_argument("--license", required=True)
    ap.add_argument("--notice", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--parts", type=int, default=2)
    a = ap.parse_args()

    os.makedirs(a.out, exist_ok=True)
    if os.listdir(a.out):
        sys.exit("output folder must be empty")

    size = os.path.getsize(a.model)
    part_bytes = -(-size // a.parts)
    if part_bytes > MAX_PART:
        sys.exit("parts would exceed 2 GiB; raise --parts")

    total, model_sha, parts = split(a.model, a.out, part_bytes)
    if total != size or model_sha != sha256_of(a.model):
        sys.exit("model changed while splitting")

    files = [{
        "name": os.path.basename(a.model), "kind": "model", "bytes": total,
        "sha256": model_sha, "license": "Apache-2.0", "parts": parts,
    }]
    for src, kind, name in [(a.mmproj, "mmproj", os.path.basename(a.mmproj)),
                            (a.license, "license", "LICENSE"),
                            (a.notice, "notice", "NOTICE")]:
        n, h = copy_hashed(src, os.path.join(a.out, name))
        if n > MAX_PART:
            sys.exit(f"{name} exceeds 2 GiB")
        files.append({"name": name, "kind": kind, "bytes": n, "sha256": h,
                      "license": "Apache-2.0", "parts": []})

    index = {"schema": 1, "model_id": f"qwen3.5-4b-miv-q4km-{model_sha[:12]}", "files": files}
    with open(os.path.join(a.out, "vlm-model-index.json"), "x", encoding="utf-8", newline="\n") as f:
        json.dump(index, f, ensure_ascii=False, indent=2)
        f.write("\n")
    print(json.dumps(index, indent=2))


if __name__ == "__main__":
    main()
