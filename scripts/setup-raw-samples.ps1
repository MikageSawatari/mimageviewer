# Downloads only the CC0 files recorded in tests/raw-samples.json.
# Python is used for HTTPS because the repository already requires it for build tooling.
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo
$code = @'
import hashlib
import json
import pathlib
import urllib.parse
import urllib.request

manifest = json.loads(pathlib.Path('tests/raw-samples.json').read_text(encoding='utf-8'))
if manifest['license_url'] != 'https://creativecommons.org/publicdomain/zero/1.0/':
    raise SystemExit('Unexpected license in sample manifest')
target = pathlib.Path('vendor/raw-samples')
target.mkdir(parents=True, exist_ok=True)
for sample in manifest['samples']:
    if sample['license_url'] != manifest['license_url']:
        raise SystemExit('Unexpected sample license: ' + sample['case'])
    if not sample['url'].startswith('https://raw.pixls.us/getfile.php/'):
        raise SystemExit('Unexpected sample URL: ' + sample['url'])
    path = target / sample['file']
    if not path.exists():
        print('Downloading ' + sample['case'] + ': ' + sample['url'], flush=True)
        partial = path.with_suffix(path.suffix + '.part')
        encoded_url = urllib.parse.quote(sample['url'], safe=':/?=&')
        with urllib.request.urlopen(encoded_url, timeout=180) as response, partial.open('wb') as out:
            while chunk := response.read(1024 * 1024):
                out.write(chunk)
        partial.replace(path)
    digest = hashlib.sha256()
    with path.open('rb') as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    actual = digest.hexdigest()
    if actual != sample['sha256']:
        raise SystemExit(f'{path}: SHA-256 mismatch: {actual} != {sample["sha256"]}')
    if sample['size'] and path.stat().st_size != sample['size']:
        raise SystemExit(f'{path}: size mismatch: {path.stat().st_size} != {sample["size"]}')
    print(f'Verified {path} ({path.stat().st_size} bytes)', flush=True)
'@
$code | python -
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
