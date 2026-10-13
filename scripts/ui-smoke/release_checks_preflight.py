"""Read stopped disposable catalogs before the initial perf folder is opened.

Never creates a database or uses APPDATA. --stopped asserts the runner's exact
portable-process refusal check; this helper is not a process terminator.
"""
import argparse
from contextlib import closing
import hashlib
import json
import shutil
import sqlite3
import sys
import uuid
from pathlib import Path

from generate_release_checks_fixture import TRAY_NAMES, isolated_path, reject_reparse

REPO = Path(__file__).resolve().parents[2]


def catalog_keys(db, scratch_root):
    wal = db.with_name(db.name + "-wal")
    reject_reparse(wal)
    if not wal.exists() or not wal.stat().st_size:
        with closing(sqlite3.connect(db.as_uri() + "?mode=ro&immutable=1", uri=True)) as conn:
            return read_keys(conn)

    # Process exit guarantees a stable DB + WAL, not that SQLite checkpointed
    # every committed transaction into the main file. Immutable ignores WAL.
    # Let SQLite recover its own WAL in a private copy, rebuilding SHM there;
    # never open the evidence/profile source in a mode that can modify it.
    with wal.open("rb") as stream:
        header = stream.read(32)
    page_size = int.from_bytes(header[8:12], "big")
    if (len(header) != 32 or header[:4] not in (b"\x37\x7f\x06\x82", b"\x37\x7f\x06\x83")
            or int.from_bytes(header[4:8], "big") != 3007000
            or page_size < 512 or page_size > 65536 or page_size & (page_size - 1)
            or (wal.stat().st_size - 32) % (page_size + 24)):
        raise ValueError(f"invalid stopped catalog WAL: {db}")
    reject_reparse(scratch_root)
    scratch_root.mkdir(parents=True, exist_ok=True)
    scratch = isolated_path(scratch_root / ("snapshot-" + uuid.uuid4().hex), scratch_root)
    scratch.mkdir()
    try:
        copied = scratch / "catalog.db"
        shutil.copyfile(db, copied)
        shutil.copyfile(wal, copied.with_name(copied.name + "-wal"))
        with closing(sqlite3.connect(copied.as_uri() + "?mode=ro", uri=True)) as conn:
            return read_keys(conn)
    finally:
        isolated_path(scratch, scratch_root)
        shutil.rmtree(scratch)


def read_keys(conn):
    return [row[0] for row in conn.execute("SELECT filename FROM thumbnails WHERE substr(filename,1,9)<>'audioart:'")]


def catalog_path(data, folder):
    # Same non-root fixture convention as catalog::db_path_for/path_key::normalize.
    text = str(folder)
    if len(text) >= 2 and text[1] == ":":
        text = text[2:]
    digest = hashlib.sha256(text.lower().replace("\\", "/").encode()).hexdigest()
    return data / "cache" / digest[:2] / (digest + ".db")


def preflight(manifest_path, data, case_id, *, stopped=False, repo=REPO):
    if not stopped:
        raise ValueError("stopped portable process check is required")
    for path in (data, manifest_path):
        reject_reparse(path)
    data = Path(data).resolve(strict=True)
    if data != (repo / "target" / "portable-smoke" / "data").resolve():
        raise ValueError("only exact disposable portable-smoke/data is allowed")
    if (data / ".disposable-smoke-data").read_text().strip() != "mimageviewer-disposable-smoke-v1;test-script=true":
        raise ValueError("disposable diagnostic marker missing")
    manifest_path = isolated_path(manifest_path, repo / "target" / "ui-smoke" / "runs")
    if manifest_path.name != "fixture-manifest.json" or manifest_path.parent.name != "fixture":
        raise ValueError("generated fixture manifest path required")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    folders = []
    for mode in ("cached", "uncached"):
        folders += [manifest["perf"][mode]["many_folders"], *manifest["perf"][mode]["navigation_order"],
                    *manifest["perf"][mode]["load_order"]]
    # Folder representative thumbnails persist in the displayed parent catalog,
    # so each tray root owns all 600 tile keys; child catalogs are not the proof.
    folders += [manifest["paths"][name] for name in TRAY_NAMES]
    opens = []
    for folder in folders:
        folder = isolated_path(Path(folder), manifest_path.parent)
        if not folder.is_dir():
            raise ValueError("fixture folder missing")
        db = catalog_path(data, folder)
        reject_reparse(db)
        keys = []
        if db.exists():
            keys = catalog_keys(db, repo / "target" / "release-check-preflight-snapshots")
        opens.append({"folder_path": str(folder), "pre_open_absent": not keys, "catalog_keys": keys})
    return {"schema_version": 1, "case_id": case_id, "stopped": True, "data_dir": str(data),
            "fixture_manifest": str(manifest_path), "opens": opens}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--data-dir", type=Path, required=True)
    parser.add_argument("--case-id", required=True)
    parser.add_argument("--json-out", type=Path, required=True)
    parser.add_argument("--stopped", action="store_true")
    args = parser.parse_args()
    try:
        output = isolated_path(args.json_out, REPO / "target" / "ui-smoke" / "runs")
        if output.name != "pre-launch-cache.json" or output.exists():
            raise ValueError("fresh pre-launch-cache.json required")
        result = preflight(args.manifest, args.data_dir, args.case_id, stopped=args.stopped)
        output.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    except (OSError, ValueError, KeyError, TypeError, sqlite3.Error) as error:
        print(f"release cache preflight unavailable: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
