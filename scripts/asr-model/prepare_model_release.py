"""Prepare the local transcription model release assets (asr-model-v1).

Copies the quantized whisper model, LICENSE and NOTICE, and writes
asr-model-index.json (schema 1) as read by the ASR addon. Nothing is uploaded;
the output folder is what a person attaches to the GitHub Release.

How the model file is made (2026-11-13, see docs/local-asr-transcript-plan.md §3):

  1. Download large-v3-turbo.pt from the URL in openai/whisper v20250625
     whisper/__init__.py (the SHA-256 is part of the URL path and is checked):
     aff26ae408abcba5fbf8813c21e62b0941638c5f6eebfb145be0c9839262a19a
  2. python vendor/whisper.cpp/models/convert-pt-to-ggml.py large-v3-turbo.pt
         <openai/whisper v20250625 source> <out dir>      (needs torch + numpy)
  3. Build whisper-quantize from vendor/whisper.cpp (v1.9.5) and run
         whisper-quantize ggml-model.bin ggml-large-v3-turbo-q5_0.bin q5_0
     The result is byte-identical to the file used in the evaluation:
     394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2

Usage:
  python scripts/asr-model/prepare_model_release.py --model <bin>
      --license <openai/whisper LICENSE> --notice scripts/asr-model/model-NOTICE.txt
      --out <empty folder>
"""

import argparse
import hashlib
import json
import os
import sys

MAX_PART = 2 * 1024 * 1024 * 1024 - 1
CHUNK = 8 * 1024 * 1024
EXPECTED_MODEL_SHA256 = "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2"


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


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True)
    ap.add_argument("--license", required=True)
    ap.add_argument("--notice", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()

    os.makedirs(a.out, exist_ok=True)
    if os.listdir(a.out):
        sys.exit("output folder must be empty")

    files = []
    for src, kind, name in [(a.model, "model", os.path.basename(a.model)),
                            (a.license, "license", "LICENSE"),
                            (a.notice, "notice", "NOTICE")]:
        n, h = copy_hashed(src, os.path.join(a.out, name))
        if n > MAX_PART:
            sys.exit(f"{name} exceeds 2 GiB")
        files.append({"name": name, "kind": kind, "bytes": n, "sha256": h,
                      "license": "MIT", "parts": []})

    model_sha = files[0]["sha256"]
    if model_sha != EXPECTED_MODEL_SHA256:
        sys.exit(f"model SHA-256 {model_sha} is not the evaluated model")
    index = {"schema": 1,
             "model_id": f"whisper-large-v3-turbo-q5_0-{model_sha[:12]}",
             "files": files}
    with open(os.path.join(a.out, "asr-model-index.json"), "x", encoding="utf-8", newline="\n") as f:
        json.dump(index, f, ensure_ascii=False, indent=2)
        f.write("\n")
    print(json.dumps(index, indent=2))


if __name__ == "__main__":
    main()
