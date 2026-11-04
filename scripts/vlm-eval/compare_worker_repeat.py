"""Compare two native workers via product --serve, sequentially, without HTTP.

Pillow is required. Images are read in place; captions are never printed.
Use --out-dir outside the repository for private JSONL, or --summary-only to
keep all generated text in memory. Existing output files are never overwritten.
"""
import argparse
import hashlib
import json
import pathlib
import statistics
import struct
import subprocess
import threading

from PIL import Image, ImageOps


class Worker:
    def __init__(self, exe, stderr, timeout):
        self.process = subprocess.Popen(
            [str(exe), "--serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=stderr, creationflags=subprocess.CREATE_NO_WINDOW,
        )
        self.timeout = timeout

    def read_exact(self, count):
        result = bytearray()
        while len(result) < count:
            part = self.process.stdout.read(count - len(result))
            if not part:
                raise RuntimeError("Worker closed the protocol pipe")
            result.extend(part)
        return result

    def call(self, header, rgb=b""):
        timer = threading.Timer(self.timeout, self.process.kill)
        timer.start()
        try:
            encoded = json.dumps(header, ensure_ascii=False).encode("utf-8")
            self.process.stdin.write(struct.pack("<II", 4 + len(encoded) + len(rgb), len(encoded)))
            self.process.stdin.write(encoded)
            self.process.stdin.write(rgb)
            self.process.stdin.flush()
            size, = struct.unpack("<I", self.read_exact(4))
            if not 4 <= size <= 16 * 1024 * 1024:
                raise RuntimeError("Invalid worker frame size")
            frame = self.read_exact(size)
            json_size, = struct.unpack_from("<I", frame)
            if json_size != size - 4:
                raise RuntimeError("Unexpected response payload")
            return json.loads(frame[4:])
        finally:
            timer.cancel()

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.close()  # EOF is the documented normal shutdown.
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
        self.process.wait()
        self.process.stdout.close()


def rgb_image(path, long_side):
    with Image.open(path) as original:
        image = ImageOps.exif_transpose(original)
        image = image.convert("RGBA")
        background = Image.new("RGBA", image.size, "white")
        background.alpha_composite(image)
        image = background.convert("RGB")
        image.thumbnail((long_side, long_side), Image.Resampling.LANCZOS)
        return image.size, image.tobytes()


def repeated_span_chars(text):
    # For each period, count equal characters at that offset backwards. Unlike
    # non-overlapping regex matches, this also finds an outer repeated block
    # containing shorter repetitions. O(n^2), bounded by worker output limits.
    best = 0
    alnum = [0]
    for char in text:
        alnum.append(alnum[-1] + char.isalnum())
    for period in range(1, len(text) // 3 + 1):
        run = 0
        for start in range(len(text) - period - 1, -1, -1):
            run = run + 1 if text[start] == text[start + period] else 0
            if run >= 2 * period and alnum[start + period] != alnum[start]:
                best = max(best, (1 + run // period) * period)
    return best


def metrics(row):
    try:
        parsed = json.loads(row.get("content", ""))
        valid_json = isinstance(parsed, dict) and isinstance(parsed.get("caption"), str) \
            and isinstance(parsed.get("text"), str) and isinstance(parsed.get("keywords"), list) \
            and all(isinstance(word, str) for word in parsed["keywords"])
    except (ValueError, TypeError):
        valid_json = False
    # A diagnostic, not a content judgment. Report aggregate lengths only;
    # whitespace-only runs are ignored. Include raw output on incomplete JSON.
    texts = [parsed["caption"], parsed["text"], *parsed["keywords"]] if valid_json else [row.get("content", "")]
    return {"status": row.get("status"), "json_valid": valid_json,
            "validation_ok": row.get("validation", {}).get("ok", False),
            "finish_reason": row.get("finish_reason"), "tokens": row.get("generated_tokens"),
            "wall_s": row.get("wall_s"),
            "repeated_span_chars": max((repeated_span_chars(text) for text in texts), default=0)}


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--baseline", required=True, type=pathlib.Path)
    ap.add_argument("--candidate", required=True, type=pathlib.Path)
    ap.add_argument("--model-dir", required=True, type=pathlib.Path)
    ap.add_argument("--image", action="append", default=[], type=pathlib.Path)
    ap.add_argument("--inputs-dir", type=pathlib.Path)
    ap.add_argument("--limit", type=int, help="limit only the sorted inputs-dir PNGs")
    ap.add_argument("--long-side", type=int, default=1024, choices=(768, 1024))
    ap.add_argument("--cpu-threads", type=int, default=24)
    ap.add_argument("--device-luid", default="")
    ap.add_argument("--timeout", type=int, default=180, help="per protocol exchange, seconds")
    output = ap.add_mutually_exclusive_group(required=True)
    output.add_argument("--out-dir", type=pathlib.Path)
    output.add_argument("--summary-only", action="store_true")
    args = ap.parse_args()
    images = args.image[:]
    if args.inputs_dir:
        selected = sorted(args.inputs_dir.glob("*.png"))
        if args.limit is not None:
            if args.limit < 1:
                ap.error("--limit must be positive")
            selected = selected[:args.limit]
        images.extend(selected)
    if not images or any(not path.is_file() for path in images):
        ap.error("Provide existing images")
    if args.out_dir:
        repo = pathlib.Path(__file__).resolve().parents[2]
        destination = args.out_dir.resolve()
        if destination == repo or repo in destination.parents:
            ap.error("Private outputs must be outside the repository")
        destination.mkdir(parents=True, exist_ok=True)
    summaries = {}
    for label, exe in (("baseline", args.baseline), ("candidate", args.candidate)):
        rows = []
        output_file = None
        stderr = subprocess.DEVNULL
        worker = None
        try:
            if args.out_dir:
                output_file = (destination / (label + ".jsonl")).open("x", encoding="utf-8")
                stderr = (destination / (label + ".stderr.log")).open("xb")
            worker = Worker(exe.resolve(), stderr, args.timeout)
            hello = worker.call({"type": "hello"})
            if hello.get("protocol_version") != 2 or hello.get("prompt_version") != 1:
                raise RuntimeError("Unexpected worker identity protocol")
            loaded = worker.call({"type": "load", "model_path": str((args.model_dir / "Qwen3.5-4B-Q4_K_M-imat.gguf").resolve()),
                                  "mmproj_path": str((args.model_dir / "mmproj-F16.gguf").resolve()),
                                  "device_luid": args.device_luid, "cpu_threads": args.cpu_threads})
            if loaded.get("status") != "ok":
                raise RuntimeError("Worker model load failed")
            input_hashes = []
            for index, path in enumerate(images, 1):
                (width, height), rgb = rgb_image(path, args.long_side)
                input_hashes.append(hashlib.sha256(struct.pack("<II", width, height) + rgb).hexdigest())
                row = worker.call({"type": "caption", "request_id": index, "width": width, "height": height}, rgb)
                if row.get("request_id") != index or row.get("type") != "caption":
                    raise RuntimeError("Unexpected caption response")
                result = metrics(row)
                rows.append(result)
                if output_file:
                    row.update(image=str(path.resolve()), source_identity=hello["source_identity"],
                               input_rgb_sha256=input_hashes[-1], model_load_s=loaded["model_load_s"],
                               device=loaded["device"])
                    output_file.write(json.dumps(row, ensure_ascii=False) + "\n")
                    output_file.flush()
                print(json.dumps({"worker": label, "index": index, **result}), flush=True)
                if row.get("status") == "fatal":
                    raise RuntimeError("Worker reported fatal caption failure")
            summaries[label] = {"source_identity": hello["source_identity"], "input_rgb_sha256": input_hashes,
                                "model_load_s": loaded["model_load_s"], "rows": rows,
                                "median_wall_s": statistics.median(row["wall_s"] for row in rows)}
        finally:
            if worker:
                worker.close()
            if output_file:
                output_file.close()
            if stderr != subprocess.DEVNULL:
                stderr.close()
    if summaries["baseline"]["input_rgb_sha256"] != summaries["candidate"]["input_rgb_sha256"]:
        raise RuntimeError("Inputs changed between workers")
    if args.out_dir:
        with (destination / "summary.json").open("x", encoding="utf-8") as out:
            json.dump(summaries, out, indent=2)
    print(json.dumps({"summary": summaries}), flush=True)
    if any(not row["validation_ok"] for run in summaries.values() for row in run["rows"]):
        raise SystemExit(2)


if __name__ == "__main__":
    main()
