"""Offline, byte-preserving import for pristine vendor checkpoints.

Run before applying local patches; existing package directories are never overwritten.
The package file inventories also identify every pristine/modified file later.
"""

import argparse
import hashlib
import json
import tarfile
from pathlib import Path

BASE = Path(__file__).resolve().parent
REGISTRY = Path.home() / ".cargo/registry"
INDEX = "index.crates.io-1949cf8c6b5b557f"


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("packages", nargs="*", default=["egui", "egui-winit"])
    args = parser.parse_args()
    provenance = BASE / "upstream-provenance.json"
    records = json.loads(provenance.read_text(encoding="utf-8")) if provenance.exists() else {}
    imported = {}
    for name in args.packages:
        if name not in ("egui", "egui-winit", "egui-wgpu"):
            raise RuntimeError(f"Unsupported package: {name}")
        package = f"{name}-0.37.0"
        source = REGISTRY / "src" / INDEX / package
        archive = REGISTRY / "cache" / INDEX / f"{package}.crate"
        index = REGISTRY / "index" / INDEX / ".cache" / "eg" / "ui" / name
        entry = next(
            json.loads(part)
            for part in index.read_bytes().split(b"\0")
            if part.startswith(b'{"name":') and json.loads(part)["vers"] == "0.37.0"
        )
        package_hash = sha256(archive.read_bytes())
        if package_hash != entry["cksum"]:
            raise RuntimeError(f"Archive/index checksum mismatch: {package}")
        destination = BASE / name
        if destination.exists():
            raise RuntimeError(f"Refusing to overwrite {destination}")
        files = {}
        with tarfile.open(archive) as tar:
            for member in tar.getmembers():
                if not member.isfile():
                    continue
                relative = Path(member.name).relative_to(package)
                if relative.is_absolute() or ".." in relative.parts:
                    raise RuntimeError(f"Unsafe package path: {member.name}")
                data = tar.extractfile(member).read()
                if (source / relative).read_bytes() != data:
                    raise RuntimeError(f"Registry/archive mismatch: {relative}")
                target = destination / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(data)
                files[relative.as_posix()] = sha256(data)
        records[name] = {
            "version": "0.37.0",
            "archive": str(archive),
            "registry_source": str(source),
            "package_sha256": package_hash,
            "checksum_reference": str(index),
            "vcs": json.loads((source / ".cargo_vcs_info.json").read_text()),
            "license": "MIT OR Apache-2.0",
            "upstream_repository": "https://github.com/emilk/egui",
            "files": files,
        }
        imported[name] = len(files)
    provenance.write_text(
        json.dumps(records, indent=2) + "\n", encoding="utf-8"
    )
    print(imported)


if __name__ == "__main__":
    main()
