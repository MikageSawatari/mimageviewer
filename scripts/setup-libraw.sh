#!/usr/bin/env bash
# Install the pinned LibRaw source and its static zlib build input.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=0.22.2
VENDOR=vendor/libraw

if [[ "${1:-}" == check ]]; then
    latest=$(gh release list --repo LibRaw/LibRaw --limit 1 --json tagName --jq '.[0].tagName')
    current=$(cat "$VENDOR/VERSION" 2>/dev/null || true)
    echo "Current LibRaw: ${current:-not installed}; latest release: $latest"
    [[ "$current" == "$latest" ]] && echo 'Up to date.' || echo 'New version available.'
    exit 0
fi

mkdir -p "$VENDOR"
python - <<'PY'
import hashlib
import pathlib
import tarfile
import urllib.request

root = pathlib.Path('vendor/libraw')
archives = (
    ('LibRaw-0.22.2.tar.gz',
     'https://github.com/LibRaw/LibRaw/archive/refs/tags/0.22.2.tar.gz',
     '627928088300ecde6ca91ffd202e189203f04ad61ad12f0fe9dc57b9a7a0fb3c', root),
    ('zlib-1.3.1.tar.gz',
     'https://github.com/madler/zlib/archive/refs/tags/v1.3.1.tar.gz',
     '17e88863f3600672ab49182f217281b6fc4d3c762bde361935e436a95214d05c',
     root / 'zlib'),
)
for name, url, expected, dest in archives:
    archive = root / name
    if not archive.exists():
        print(f'Downloading {url}', flush=True)
        with urllib.request.urlopen(url, timeout=120) as response:
            archive.write_bytes(response.read())
    actual = hashlib.sha256(archive.read_bytes()).hexdigest()
    if actual != expected:
        raise SystemExit(f'{archive}: SHA-256 mismatch: {actual} != {expected}')
    with tarfile.open(archive, 'r:gz') as source:
        for member in source:
            parts = pathlib.PurePosixPath(member.name).parts
            if len(parts) < 2 or not (member.isdir() or member.isfile()):
                continue
            relative = pathlib.PurePosixPath(*parts[1:])
            if '..' in relative.parts or relative.is_absolute():
                raise SystemExit(f'Unsafe archive path: {member.name}')
            member.name = str(relative)
            source.extract(member, dest)
    print(f'Verified and extracted {name}', flush=True)
(root / 'VERSION').write_text('0.22.2\n', encoding='ascii')
PY
