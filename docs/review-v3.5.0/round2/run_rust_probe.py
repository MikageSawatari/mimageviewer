"""Link a review-only Rust probe with existing checked production libraries."""
from pathlib import Path
import os
import json
import subprocess
import sys

root = Path(__file__).resolve().parents[3]
source = Path(sys.argv[1]).resolve()
output = root / "target/review-v350-round2" / (source.stem + ".exe")
deps = root / "target/debug/deps"
production = max(deps.glob("libmimageviewer-*.rlib"), key=lambda p: p.stat().st_mtime)
fingerprints = root / "target/debug/.fingerprint"
metadata = json.loads((fingerprints / production.stem[3:] / "lib-mimageviewer.json").read_text())
expected = {entry[1]: entry[3].to_bytes(8, "little").hex() for entry in metadata["deps"]}
command = ["rustc", "--edition=2024", str(source), "-o", str(output), "-C", "linker=rust-lld.exe",
           "-C", "target-feature=+crt-static", "-L", f"dependency={deps}", "-L", f"native={root / 'vendor/ffmpeg/lib'}"]
for name in sys.argv[2:]:
    if name == "mimageviewer":
        library = production
    else:
        matches = [p for p in fingerprints.glob(f"*/lib-{name}") if p.read_text().strip() == expected[name]]
        assert len(matches) == 1, (name, matches)
        suffix = matches[0].parent.name.rsplit("-", 1)[1]
        library = deps / f"lib{name}-{suffix}.rlib"
    command += ["--extern", f"{name}={library}"]
for native in (root / "target/debug/build").glob("*/out"):
    if any(native.glob("*.lib")) or any(native.glob("*.a")):
        command += ["-L", f"native={native}"]
cargo_root = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
for native in (cargo_root / "registry/src").glob("*/windows_x86_64_msvc-*/lib"):
    command += ["-L", f"native={native}"]
compiled = subprocess.run(command, cwd=root, capture_output=True, text=True)
if compiled.returncode:
    source.with_suffix(".compile.log").write_text(compiled.stdout + compiled.stderr, encoding="utf-8")
    print("\n".join(line for line in compiled.stderr.splitlines() if "error" in line or "-->" in line))
    sys.exit(compiled.returncode)
env = os.environ.copy()
env["PATH"] = str(root / "vendor/ffmpeg/bin") + os.pathsep + env.get("PATH", "")
result = subprocess.run([str(output)], cwd=root, env=env, capture_output=True, text=True)
source.with_suffix(".log").write_text(result.stdout + result.stderr, encoding="utf-8")
print(result.stdout + result.stderr, end="")
result.check_returncode()
