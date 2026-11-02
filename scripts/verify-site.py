"""Compare the public mikage.to site with the local htdocs/ tree.

Usage: python scripts/verify-site.py htdocs https://www.mikage.to
Fetches every local file from the site and compares SHA-256. A 403 on
.htaccess is expected (the server does not serve it).
Exit code: 0 when everything matches (ignoring .htaccess), 1 otherwise.
"""
import hashlib, sys, urllib.parse, urllib.request, urllib.error
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

root = Path(sys.argv[1])
base = sys.argv[2].rstrip('/')
files = [p for p in root.rglob('*') if p.is_file()]

def check(p):
    rel = p.relative_to(root).as_posix()
    url = base + '/' + urllib.parse.quote(rel)
    try:
        req = urllib.request.Request(url, headers={'User-Agent': 'miv-site-verify', 'Cache-Control': 'no-cache'})
        with urllib.request.urlopen(req, timeout=60) as r:
            body = r.read()
        same = hashlib.sha256(body).digest() == hashlib.sha256(p.read_bytes()).digest()
        return rel, 'ok' if same else 'DIFF'
    except urllib.error.HTTPError as e:
        return rel, f'HTTP {e.code}'
    except Exception as e:
        return rel, f'ERR {e}'

with ThreadPoolExecutor(8) as ex:
    res = list(ex.map(check, files))
bad = [r for r in res if r[1] != 'ok']
print(f'total={len(res)} ok={len(res) - len(bad)} bad={len(bad)}')
for rel, st in bad:
    print(st, rel)
sys.exit(1 if any(not rel.endswith('.htaccess') for rel, _ in bad) else 0)
