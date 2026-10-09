"""Read stopped disposable catalogs before the initial perf folder is opened.

Never creates a database or uses APPDATA. --stopped asserts the runner's exact
portable-process refusal check; this helper is not a process terminator.
"""
import argparse
from contextlib import closing
import hashlib
import json
import sqlite3
import sys
from pathlib import Path

from generate_release_checks_fixture import isolated_path, reject_reparse

REPO = Path(__file__).resolve().parents[2]


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
        folders += [manifest["perf"][mode]["many_folders"], *manifest["perf"][mode]["navigation_order"]]
    opens = []
    for folder in folders:
        folder = isolated_path(Path(folder), manifest_path.parent)
        if not folder.is_dir():
            raise ValueError("fixture folder missing")
        db = catalog_path(data, folder)
        reject_reparse(db)
        keys = []
        if db.exists():
            # Immutable avoids DB/SHM changes. A live/persisted WAL cannot be
            # ignored as empty cache; require a fully flushed stopped catalog.
            wal = db.with_name(db.name + "-wal")
            reject_reparse(wal)
            if wal.exists() and wal.stat().st_size:
                raise ValueError("nonempty stopped catalog WAL: cache proof unavailable")
            with closing(sqlite3.connect(db.as_uri() + "?mode=ro&immutable=1", uri=True)) as conn:
                keys = [row[0] for row in conn.execute("SELECT filename FROM thumbnails WHERE substr(filename,1,9)<>'audioart:'")]
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
