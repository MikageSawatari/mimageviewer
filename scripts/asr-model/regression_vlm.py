"""Compare real-model VLM behavior before/after adding the ASR worker role.

Run `run --worker EXE --label before|after --model GGUF --mmproj GGUF
--images DIR --out-dir DIR`, then `compare --out-dir DIR`. Use the shared
run-heavy.ps1 runner for inference. Only native AI workers are launched.
Private JSON/JSONL stays in the explicitly selected output directory. Stdout
and summary.json contain counts and hashes, never image names or output text.
Existing evidence is never overwritten. No image/model copies are made.
"""

import argparse
import hashlib
import json
import pathlib
import struct
import subprocess
import threading
import zlib


SEMANTIC_FIELDS = ("caption", "text", "keywords", "content", "status", "finish_reason", "generated_tokens")
FIELDS = SEMANTIC_FIELDS + ("usage", "prompt_chunks", "validation")
MAX_FRAME = 16 * 1024 * 1024


def write_json(path, value):
    with path.open("x", encoding="utf-8") as output:
        json.dump(value, output, ensure_ascii=False, indent=2)
        output.write("\n")


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def png_rgb(path):
    """Decode only bounded, noninterlaced RGB8 PNG fixtures without Pillow.

    Evaluation inputs are already 1024px RGB8. Reject other PNG formats instead
    of silently changing pixels. This decoder is verification-only, not product
    image handling. Both workers receive the same byte buffer and dimensions.
    """
    with path.open("rb") as source:
        encoded = source.read(MAX_FRAME + 1)
    if len(encoded) > MAX_FRAME or encoded[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("Expected a bounded PNG fixture")
    position = 8
    compressed = bytearray()
    width = height = None
    ended = False
    while position + 12 <= len(encoded):
        size = struct.unpack_from(">I", encoded, position)[0]
        kind = encoded[position + 4:position + 8]
        end = position + 12 + size
        if end > len(encoded):
            raise ValueError("Truncated PNG fixture")
        payload = encoded[position + 8:end - 4]
        crc = struct.unpack_from(">I", encoded, end - 4)[0]
        if zlib.crc32(kind + payload) != crc:
            raise ValueError("PNG fixture CRC mismatch")
        if kind == b"IHDR":
            if width is not None or position != 8 or size != 13:
                raise ValueError("Invalid PNG IHDR")
            width, height, depth, color, compression, filtering, interlace = struct.unpack(">IIBBBBB", payload)
            if not (1 <= width <= 1024 and 1 <= height <= 1024) or (
                depth, color, compression, filtering, interlace
            ) != (8, 2, 0, 0, 0):
                raise ValueError("Verification requires noninterlaced RGB8 PNG, dimensions <=1024")
        elif kind == b"IDAT":
            compressed.extend(payload)
        elif kind == b"IEND":
            ended = True
            break
        elif kind == b"tRNS":
            raise ValueError("Verification requires opaque RGB8 PNG")
        position = end
    if width is None or not ended:
        raise ValueError("Incomplete PNG fixture")
    stride = width * 3
    expected = (stride + 1) * height
    decoder = zlib.decompressobj()
    raw = decoder.decompress(compressed, expected + 1)
    if len(raw) != expected or not decoder.eof or decoder.unused_data:
        raise ValueError("Invalid PNG decompressed size")
    pixels = bytearray()
    previous = bytearray(stride)
    for row_index in range(height):
        start = row_index * (stride + 1)
        filter_type = raw[start]
        if filter_type > 4:
            raise ValueError("Invalid PNG filter")
        row = bytearray(raw[start + 1:start + 1 + stride])
        for index in range(stride):
            left = row[index - 3] if index >= 3 else 0
            up = previous[index]
            upper_left = previous[index - 3] if index >= 3 else 0
            if filter_type == 1:
                predictor = left
            elif filter_type == 2:
                predictor = up
            elif filter_type == 3:
                predictor = (left + up) // 2
            elif filter_type == 4:
                estimate = left + up - upper_left
                distances = (abs(estimate - left), abs(estimate - up), abs(estimate - upper_left))
                predictor = (left, up, upper_left)[distances.index(min(distances))]
            else:
                predictor = 0
            row[index] = (row[index] + predictor) & 255
        pixels.extend(row)
        previous = row
    return width, height, bytes(pixels)


class Worker:
    def __init__(self, exe, stderr, timeout):
        self.process = subprocess.Popen(
            [str(exe), "--serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=stderr, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
        self.timeout = timeout

    def read_exact(self, size):
        output = bytearray()
        while len(output) < size:
            chunk = self.process.stdout.read(size - len(output))
            if not chunk:
                raise RuntimeError("VLM worker closed its protocol pipe")
            output.extend(chunk)
        return output

    def call(self, header, payload=b""):
        timer = threading.Timer(self.timeout, self.process.kill)
        timer.start()
        try:
            encoded = json.dumps(header).encode("utf-8")
            self.process.stdin.write(struct.pack("<II", 4 + len(encoded) + len(payload), len(encoded)))
            self.process.stdin.write(encoded)
            self.process.stdin.write(payload)
            self.process.stdin.flush()
            size, = struct.unpack("<I", self.read_exact(4))
            if not 4 <= size <= MAX_FRAME:
                raise RuntimeError("Invalid VLM response frame length")
            frame = self.read_exact(size)
            json_size, = struct.unpack_from("<I", frame)
            if json_size != size - 4:
                raise RuntimeError("Unexpected VLM response payload")
            response = json.loads(frame[4:].decode("utf-8"))
            if not isinstance(response, dict):
                raise RuntimeError("Expected VLM object response")
            return response
        finally:
            timer.cancel()

    def close(self):
        try:
            if self.process.poll() is None:
                header = json.dumps({"type": "shutdown"}).encode("utf-8")
                self.process.stdin.write(struct.pack("<II", 4 + len(header), len(header)) + header)
                self.process.stdin.flush()
                self.process.stdin.close()
            self.process.wait(timeout=15)
        except (BrokenPipeError, OSError, subprocess.TimeoutExpired):
            self.process.kill()
            self.process.wait()
        finally:
            self.process.stdout.close()


def rows(path):
    data = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]
    keyed = {row["image"]: row for row in data}
    if not data or len(data) != len(keyed):
        raise ValueError("Selftest image set is empty or contains duplicates")
    return keyed


def valid(row):
    return (all(field in row for field in FIELDS) and row["status"] == "ok"
            and row.get("validation", {}).get("ok") is True)


def run(args):
    out = args.out_dir.resolve()
    out.mkdir(parents=True, exist_ok=True)
    prefix = out / args.label
    worker_path, model, mmproj, images_dir = (path.resolve() for path in
                                             (args.worker, args.model, args.mmproj, args.images))
    images = sorted((p for p in images_dir.iterdir() if p.is_file() and p.suffix.lower() == ".png"),
                    key=lambda path: path.name.encode("utf-8"))
    if len(images) != args.expected_count:
        raise ValueError("Unexpected selftest PNG count")
    inputs = {path.name: sha256(path) for path in images}
    width, height, rgb = png_rgb(images[0])
    manifest = {"worker_sha256": sha256(worker_path), "input_sha256": inputs,
                "serve_rgb_sha256": hashlib.sha256(struct.pack("<II", width, height) + rgb).hexdigest(),
                "model_sha256": sha256(model), "mmproj_sha256": sha256(mmproj)}
    command = [str(worker_path), "--selftest", "--model", str(model), "--mmproj", str(mmproj),
               "--images", str(images_dir), "--out", str(prefix) + "-forward.jsonl", "--order", "forward"]
    write_json(pathlib.Path(str(prefix) + "-command.json"), command)
    with pathlib.Path(str(prefix) + "-selftest.stdout.log").open("xb") as stdout, \
         pathlib.Path(str(prefix) + "-selftest.stderr.log").open("xb") as stderr:
        completed = subprocess.run(command, stdout=stdout, stderr=stderr, timeout=args.timeout)
    manifest["selftest_exit"] = completed.returncode
    if completed.returncode != 0:
        write_json(pathlib.Path(str(prefix) + "-manifest.json"), manifest)
        raise RuntimeError("VLM selftest failed; inspect private logs")
    forward = rows(pathlib.Path(str(prefix) + "-forward.jsonl"))
    if len(forward) != args.expected_count or not all(valid(row) for row in forward.values()):
        raise RuntimeError("VLM selftest did not validate all expected images")
    with pathlib.Path(str(prefix) + "-serve.stderr.log").open("xb") as stderr:
        worker = Worker(worker_path, stderr, args.exchange_timeout)
        try:
            hello = worker.call({"type": "hello"})
            if (hello.get("type"), hello.get("protocol_version"), hello.get("prompt_version")) != ("hello", 2, 1):
                raise RuntimeError("VLM protocol 2 hello mismatch")
            loaded = worker.call({"type": "load", "model_path": str(model), "mmproj_path": str(mmproj),
                                  "device_luid": "", "cpu_threads": 24})
            if loaded.get("type") != "load" or loaded.get("status") != "ok":
                raise RuntimeError("VLM protocol 2 model load failed")
            caption = worker.call({"type": "caption", "request_id": 1, "width": width, "height": height}, rgb)
            if caption.get("type") != "caption" or caption.get("request_id") != 1 or not valid(caption):
                raise RuntimeError("VLM protocol 2 caption failed")
            write_json(pathlib.Path(str(prefix) + "-serve.json"), {"hello": hello, "load": loaded, "caption": caption})
        finally:
            worker.close()
    manifest["serve_exit"] = worker.process.returncode
    manifest["source_identity"] = hello.get("source_identity")
    manifest["count"] = len(forward)
    if manifest["serve_exit"] != 0:
        raise RuntimeError("VLM protocol 2 shutdown failed")
    if inputs != {path.name: sha256(path) for path in images}:
        raise RuntimeError("VLM input fixtures changed during inference")
    write_json(pathlib.Path(str(prefix) + "-manifest.json"), manifest)
    print(json.dumps({"label": args.label, "selftest_count": len(forward), "validation_ok": len(forward),
                      "serve_count": 1, "serve_validation_ok": 1, "worker_sha256": manifest["worker_sha256"]}), flush=True)


def compare(args):
    out = args.out_dir.resolve()
    manifests = [json.loads((out / (label + "-manifest.json")).read_text(encoding="utf-8")) for label in ("before", "after")]
    before, after = [rows(out / (label + "-forward.jsonl")) for label in ("before", "after")]
    if before.keys() != after.keys() or len(before) != args.expected_count:
        raise ValueError("VLM comparison has mismatched image sets or count")
    same_inputs = all(manifests[0][key] == manifests[1][key] for key in
                      ("input_sha256", "serve_rgb_sha256", "model_sha256", "mmproj_sha256"))
    counts = {field: sum(before[name].get(field) == after[name].get(field) for name in before) for field in FIELDS}
    serve_runs = [json.loads((out / (label + "-serve.json")).read_text(encoding="utf-8")) for label in ("before", "after")]
    served = [run["caption"] for run in serve_runs]
    serve_equal = {field: served[0].get(field) == served[1].get(field) for field in FIELDS}
    validated = [sum(valid(row) for row in run_rows.values()) for run_rows in (before, after)]
    device_equal = sum(before[name].get("device") == after[name].get("device") for name in before)
    same_serve_device = serve_runs[0]["load"].get("device") == serve_runs[1]["load"].get("device")
    first_image = next(iter(before))
    # prompt_chunks の診断表現はselftest/serveで異なるため、モード間は内容の7項目を照合する。
    serve_matches_selftest = [all(run_rows[first_image].get(field) == caption.get(field) for field in SEMANTIC_FIELDS)
                             for run_rows, caption in zip((before, after), served)]
    stable_fields_present = all(all(field in row for field in FIELDS)
                               for row in [*before.values(), *after.values(), *served])
    result = {"selftest_count": len(before), "selftest_validated_before_after": validated,
              "field_equal": counts, "stable_fields_present": stable_fields_present,
              "serve_count": 1, "serve_field_equal": serve_equal,
              "serve_validated_before_after": [valid(row) for row in served], "same_inputs": same_inputs,
              "device_equal_count": device_equal, "serve_device_equal": same_serve_device,
              "serve_matches_first_selftest_before_after": serve_matches_selftest,
              "worker_sha256_before_after": [manifest["worker_sha256"] for manifest in manifests]}
    result["passed"] = (same_inputs and stable_fields_present and validated == [len(before), len(before)]
                        and all(count == len(before) for count in counts.values())
                        and all(serve_equal.values()) and all(valid(row) for row in served)
                        and device_equal == len(before) and same_serve_device and all(serve_matches_selftest))
    write_json(out / "summary.json", result)
    print(json.dumps(result, indent=2))
    if not result["passed"]:
        raise SystemExit(1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    runner = commands.add_parser("run")
    for name in ("worker", "model", "mmproj", "images"):
        runner.add_argument("--" + name, required=True, type=pathlib.Path)
    runner.add_argument("--label", required=True, choices=("before", "after"))
    runner.add_argument("--timeout", type=int, default=600)
    runner.add_argument("--exchange-timeout", type=int, default=180)
    comparator = commands.add_parser("compare")
    for command in (runner, comparator):
        command.add_argument("--out-dir", required=True, type=pathlib.Path)
        command.add_argument("--expected-count", type=int, default=26)
    args = parser.parse_args()
    (run if args.command == "run" else compare)(args)


if __name__ == "__main__":
    main()
