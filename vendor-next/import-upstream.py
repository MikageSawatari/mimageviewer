"""Offline, byte-preserving import for the stage 2a-1 pristine checkpoint.

Run before applying local patches, in a checkout without vendor-next/egui*.
The package file inventories also identify every pristine/modified file later.
"""

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
    records = {}
    for name in ("egui", "egui-winit"):
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
    (BASE / "upstream-provenance.json").write_text(
        json.dumps(records, indent=2) + "\n", encoding="utf-8"
    )
    print({name: len(record["files"]) for name, record in records.items()})


if __name__ == "__main__":
    main()
