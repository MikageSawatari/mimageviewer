"""Legacy FFmpeg allocation repro. Run: python scripts/check_audio_album_art_gate.py

Preserves the failed open-only gate evidence in docs/audio-album-art-plan.md
section 15, also informing the separate playback observation in section 16.
The redesigned album-art reader excludes FFmpeg; this is not its acceptance gate.
The child is a dependency harness, not a product binary. All input is synthetic;
DLLs are loaded from this worktree. The oversized compressed APIC should still
fail the old allocation gate (nonzero result); do not reinterpret it as passing.
"""
from pathlib import Path
from io import BytesIO
import hashlib
import json
import os
import subprocess
import sys
import zlib
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "target/audio-art-gate"
OUT.mkdir(parents=True, exist_ok=True)

def build_harness():
    """Build a dependency-only executable; never link or launch mImageViewer."""
    installer = Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")) / "Microsoft Visual Studio/Installer"
    vswhere = installer / "vswhere.exe"
    installation = subprocess.check_output(
        [str(vswhere), "-latest", "-products", "*", "-requires",
         "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"],
        text=True, encoding="utf-8").strip()
    if not installation:
        raise RuntimeError("MSVC x64 build tools are required for the standalone gate")
    setup = Path(installation) / "VC/Auxiliary/Build/vcvars64.bat"
    # Paths below come only from the repository and the installed compiler.
    for path in (ROOT, installer, setup):
        if any(ch in str(path) for ch in '\"\r\n%'):
            raise RuntimeError("Unsupported build path")
    build = OUT / "build.cmd"
    commands = [
        "@echo off",
        f'set "PATH={installer};%PATH%"',
        f'call "{setup}" >nul',
        "if errorlevel 1 exit /b %errorlevel%",
        "cl /nologo /O2 /W3 /utf-8 /D_CRT_SECURE_NO_WARNINGS /I vendor\\ffmpeg\\include "
        "/Fo:target\\audio-art-gate\\gate.obj /Fe:target\\audio-art-gate\\gate.exe "
        "scripts\\audio_album_art_gate.c /link /LIBPATH:vendor\\ffmpeg\\lib avformat.lib avutil.lib psapi.lib",
    ]
    build.write_bytes(("\r\n".join(commands) + "\r\n").encode("utf-8"))
    proc = subprocess.run([os.environ.get("COMSPEC", "cmd.exe"), "/d", "/c", str(build)],
                          cwd=ROOT, capture_output=True, timeout=120)
    (OUT / "build.stdout.txt").write_bytes(proc.stdout)
    (OUT / "build.stderr.txt").write_bytes(proc.stderr)
    if proc.returncode:
        raise RuntimeError(f"Gate build failed ({proc.returncode}); see {OUT}")

build_harness()
MIB = 1024 * 1024
READ_LIMIT = 32 * MIB + 65536

def syncsafe(n):
    assert 0 <= n < 1 << 28
    return bytes((n >> 21 & 127, n >> 14 & 127, n >> 7 & 127, n & 127))

def frame(body, version=3, flags=0):
    if version == 2:
        return b"PIC" + len(body).to_bytes(3, "big") + body
    size = syncsafe(len(body)) if version == 4 else len(body).to_bytes(4, "big")
    return b"APIC" + size + flags.to_bytes(2, "big") + body

def tag(body, version=3, flags=0):
    return b"ID3" + bytes((version, 0, flags)) + syncsafe(len(body)) + body

def image(fmt):
    buf = BytesIO()
    Image.new("RGB", (1, 1), (200, 100, 50)).save(buf, format=fmt)
    return buf.getvalue()

PNG = image("PNG")
JPEG = image("JPEG")
AUDIO = (bytes.fromhex("fffb9000") + bytes(413)) * 32

def apic(data, mime=b"image/png", picture_type=3, description=b"front"):
    return b"\0" + mime + b"\0" + bytes((picture_type,)) + description + b"\0" + data

def fixture(name, data):
    p = OUT / (name + ".mp3")
    p.write_bytes(data + AUDIO)
    return p

cases = []
cases.append(("v23-jpeg", fixture("v23-jpeg", tag(frame(apic(JPEG, b"image/jpeg")))), -1, READ_LIMIT, 10000,
              lambda r: r["ret"] >= 0 and r["pictures"] == 1 and r["front"] == 1))
cases.append(("v24-png", fixture("v24-png", tag(frame(apic(PNG), 4), 4)), -1, READ_LIMIT, 10000,
              lambda r: r["ret"] >= 0 and r["pictures"] == 1 and r["front"] == 1))
cases.append(("v22-pic", fixture("v22-pic", tag(frame(b"\0PNG\3front\0" + PNG, 2), 2)), -1, READ_LIMIT, 10000,
              lambda r: r["ret"] >= 0 and r["pictures"] == 1 and r["front"] == 1))
several = tag(frame(apic(PNG, picture_type=4, description=b"back")) +
              frame(apic(PNG, description=b"front-a")) + frame(apic(PNG, description=b"front-b")))
cases.append(("several-pictures", fixture("several-pictures", several), -1, READ_LIMIT, 10000,
              lambda r: r["ret"] >= 0 and r["pictures"] == 3 and r["front"] == 2))
normal = cases[0][1]
cases.append(("no-art", fixture("no-art", b""), -1, READ_LIMIT, 10000,
              lambda r: r["ret"] >= 0 and r["pictures"] == 0))
cases.append(("cancel-before-open", normal, 0, READ_LIMIT, 10000,
              lambda r: r["preflight"] < 0 and r["reason"] == 1 and r["bytes_read"] == 0))
cases.append(("deadline-before-open", normal, -1, READ_LIMIT, 0,
              lambda r: r["preflight"] < 0 and r["reason"] == 2 and r["bytes_read"] == 0))
cases.append(("cancel-during-read", normal, 64, READ_LIMIT, 10000,
              lambda r: r["ret"] < 0 and r["reason"] == 1 and r["stop_callbacks"] > 0))
cases.append(("cumulative-read-limit", normal, -1, 64, 10000,
              lambda r: r["ret"] < 0 and r["bytes_read"] <= 64 and r["reason"] == 3))
# Two independent physical tags whose aggregate exceeds 32 MiB. Sparse files
# keep the fixture cheap; neither header alone exceeds the configured bound.
multiple = OUT / "multiple-tags-over-limit.mp3"
with multiple.open("wb") as f:
    for _ in range(2):
        f.write(b"ID3\3\0\0" + syncsafe(17 * MIB))
        f.seek(17 * MIB, 1)
    f.write(AUDIO)
cases.append(("multiple-tags-over-limit", multiple, -1, READ_LIMIT, 10000,
              lambda r: r["preflight"] < 0 and r["headers"] == 1 and r["bytes_read"] == 0))
# ID3v2.4 compressed APIC: valid syncsafe frame length and data-length indicator.
# Generate 24 MiB of expanded APIC bytes incrementally, without a large Python
# allocation. PNG dimensions remain 1x1. The harness never decodes the image.
expanded = 24 * MIB
prefix = apic(PNG)
compressor = zlib.compressobj()
parts = [compressor.compress(prefix)]
remaining = expanded - len(prefix)
chunk = bytes(65536)
while remaining:
    count = min(remaining, len(chunk))
    parts.append(compressor.compress(chunk[:count]))
    remaining -= count
parts.append(compressor.flush())
compressed_body = syncsafe(expanded) + b"".join(parts)
compressed = fixture("compressed-apic", tag(frame(compressed_body, 4, 0x0009), 4))
cases.append(("compressed-allocation-limit", compressed, -1, READ_LIMIT, 10000,
              lambda r: r["preflight"] < 0 or (r["largest_picture"] <= 16 * MIB and
                                               r["peak_private_delta"] <= 160 * MIB)))

env = os.environ.copy()
env["PATH"] = str(ROOT / "vendor/ffmpeg/bin") + os.pathsep + env.get("PATH", "")
results = []
for name, path, cancel, limit, deadline, predicate in cases:
    proc = subprocess.run([str(OUT / "gate.exe"), str(path), str(cancel), str(limit), str(deadline)],
                          env=env, capture_output=True, text=True, timeout=20)
    (OUT / (name + ".stderr.txt")).write_text(proc.stderr, encoding="utf-8")
    if proc.returncode:
        row = {"case": name, "passed": False, "process_exit": proc.returncode}
    else:
        row = json.loads(proc.stdout)
        row.update(case=name, passed=bool(predicate(row)), process_exit=proc.returncode)
    results.append(row)
    print(json.dumps(row, sort_keys=True))
report = {
    "command": "python scripts/check_audio_album_art_gate.py",
    "head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
    "harness_sha256": hashlib.sha256((ROOT / "scripts/audio_album_art_gate.c").read_bytes()).hexdigest(),
    "dll_sha256": {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                   for p in (ROOT / "vendor/ffmpeg/bin").glob("*.dll")},
    "cases": results,
    "passed": sum(r["passed"] for r in results),
    "failed": sum(not r["passed"] for r in results),
}
(OUT / "results.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
print(f"Gate: {report['passed']} passed, {report['failed']} failed")
sys.exit(1 if report["failed"] else 0)
