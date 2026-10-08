"""Run the bounded reader gate without starting mImageViewer; retain exit/allocator evidence."""
import hashlib
import json
import re
import subprocess
from pathlib import Path

root = Path(__file__).resolve().parents[2]
out = root / "target/audio-art-rust-gate"
out.mkdir(parents=True, exist_ok=True)
command = ["cargo", "test", "--manifest-path", "scripts/audio-art-rust-gate/Cargo.toml",
           "--offline", "--target-dir", "target/audio-art-rust-gate/build",
           "--", "--nocapture", "--test-threads=1"]
result = subprocess.run(command, cwd=root, stdout=subprocess.PIPE,
                        stderr=subprocess.STDOUT, timeout=900)
log = result.stdout.decode("utf-8", errors="replace")
(out / "results.log").write_text(log, encoding="utf-8", newline="\r\n")
counts = re.search(r"test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored", log)
files = ["src/audio_album_art.rs", "src/audio_album_art/tests.rs",
         "scripts/audio-art-rust-gate/src/main.rs", "scripts/audio-art-rust-gate/Cargo.lock"]
report = {
    "command": command, "exit_code": result.returncode,
    "rustc": subprocess.check_output(["rustc", "-Vv"], cwd=root).decode().strip(),
    "source_sha256": {name: hashlib.sha256((root / name).read_bytes()).hexdigest()
                      for name in files},
    "tests": dict(zip(["status", "passed", "failed", "ignored"], counts.groups()))
             if counts else None,
    "allocations": [dict(zip(["max_request", "peak_live", "calls"], map(int, match)))
                    for match in re.findall(r"GATE allocation max_request=(\d+) peak_live=(\d+) calls=(\d+)", log)],
    "scope": "Exact product parser, leading tag only; allocator measurements exclude fixture creation."
}
(out / "results.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
print(json.dumps({"exit_code": result.returncode, "tests": report["tests"], "report": str(out / "results.json")}))
raise SystemExit(result.returncode)
