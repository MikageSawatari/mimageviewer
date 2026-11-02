"""Audit all shipped PE files using MSVC dumpbin, without executing them."""
import argparse
import hashlib
import json
import pathlib
import re
import subprocess


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--dumpbin", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    output = pathlib.Path(a.out)
    output.mkdir(parents=True, exist_ok=True)
    blocked = {"ws2_32.dll", "wsock32.dll", "winhttp.dll", "wininet.dll", "urlmon.dll", "dnsapi.dll",
               "mswsock.dll", "iphlpapi.dll", "netapi32.dll", "wldap32.dll", "rasapi32.dll"}
    files = sorted(p for p in pathlib.Path(a.bin).rglob("*") if p.suffix.lower() in (".exe", ".dll"))
    if not files:
        raise ValueError("No shipped PE files")
    report = []
    for p in files:
        result = subprocess.run([a.dumpbin, "/nologo", "/imports", str(p.resolve())], capture_output=True, check=True)
        text = result.stdout.decode("mbcs", errors="replace")
        imports = sorted(set(re.findall(r"^\s+([\w.\-]+\.dll)\s*$", text, re.IGNORECASE | re.MULTILINE)))
        if not imports:
            raise ValueError("Could not parse dumpbin imports")
        (output / (p.name + ".imports.txt")).write_text(text, encoding="utf-8")
        report.append({"file": p.name, "bytes": p.stat().st_size, "sha256": hashlib.sha256(p.read_bytes()).hexdigest(),
                       "imports": imports, "network_imports": sorted(blocked & {s.lower() for s in imports})})
    (output / "imports.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))
    if any(r["network_imports"] for r in report):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
