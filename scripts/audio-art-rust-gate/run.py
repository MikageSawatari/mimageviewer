"""Run the bounded reader gate without starting mImageViewer; retain exit/allocator evidence."""
import hashlib
import json
import re
import subprocess
from PIL import Image, __version__ as pillow_version
from pathlib import Path

root = Path(__file__).resolve().parents[2]
out = root / "target/audio-art-rust-gate"
out.mkdir(parents=True, exist_ok=True)
fixture = out / "fixtures/progressive-8000x5000-444.jpg"
fixture.parent.mkdir(parents=True, exist_ok=True)
Image.new("RGB", (8000, 5000), (20, 100, 220)).save(
    fixture, format="JPEG", progressive=True, subsampling=0, quality=75)
command = ["cargo", "test", "--manifest-path", "scripts/audio-art-rust-gate/Cargo.toml",
           "--offline", "--target-dir", "target/audio-art-rust-gate/build",
           "--", "--nocapture", "--test-threads=1"]
result = subprocess.run(command, cwd=root, stdout=subprocess.PIPE,
                        stderr=subprocess.STDOUT, timeout=900)
log = result.stdout.decode("utf-8", errors="replace")
(out / "results.log").write_text(log, encoding="utf-8", newline="\r\n")
counts = re.search(r"test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored", log)
files = ["src/audio_album_art.rs", "src/audio_album_art/tests.rs", "src/audio_album_art/fuzz.rs",
         "src/audio_thumbnail/metadata.rs", "src/audio_thumbnail/metadata_tests.rs",
         "scripts/audio-art-rust-gate/src/main.rs", "scripts/audio-art-rust-gate/Cargo.lock"]
report = {
    "command": command, "exit_code": result.returncode,
    "rustc": subprocess.check_output(["rustc", "-Vv"], cwd=root).decode().strip(),
    "source_sha256": {name: hashlib.sha256((root / name).read_bytes()).hexdigest()
                      for name in files},
    "codec_fixture": {"path": str(fixture), "sha256": hashlib.sha256(fixture.read_bytes()).hexdigest(),
                      "bytes": fixture.stat().st_size, "dimensions": [8000, 5000],
                      "progressive": True, "subsampling": "4:4:4", "pillow": pillow_version},
    "tests": dict(zip(["status", "passed", "failed", "ignored"], counts.groups()))
             if counts else None,
    "allocations": [dict(zip(["max_request", "peak_live", "calls"], map(int, match)))
                    for match in re.findall(r"GATE allocation max_request=(\d+) peak_live=(\d+) calls=(\d+)", log)],
    "randomized": [dict(zip(["iterations", "seed", "max_request", "peak_live"], match))
                   for match in re.findall(r"GATE randomized iterations=(\d+) seed=([0-9a-f]+) max_request=(\d+) peak_live=(\d+)", log)],
    "toolchain": subprocess.run(["rustup", "toolchain", "list"], cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT).stdout.decode().strip(),
    "sanitizer_probe": subprocess.run(["rustc", "-Z", "help"], cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT).stdout.decode().strip(),
    "cargo_fuzz_probe": subprocess.run(["cargo", "fuzz", "--help"], cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT).stdout.decode().strip(),
    "scope": "Exact product leading-tag parser and allocation-free album-art EXIF/JPEG preflight. 1,200,000 deterministic arbitrary-input/multiformat seed cases with injected small parser/decoder limits; stable MSVC has no sanitizer/cargo-fuzz run. Allocator measurements exclude fixture creation. Native decoder allocations are bounded by common preflight, not intercepted by Rust allocator."
}
(out / "results.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
print(json.dumps({"exit_code": result.returncode, "tests": report["tests"], "report": str(out / "results.json")}))
raise SystemExit(result.returncode)
