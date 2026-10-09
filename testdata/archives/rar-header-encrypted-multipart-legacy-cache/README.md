# Header-encrypted multipart RAR legacy-cache regression

Generated locally with WinRAR RAR 7.22 x64 (1 May 2026), using only a
synthetic 128 x 128 RGB PNG. There are no user files or private passwords.

Public test password: `public-mimageviewer-rar-test`

Reproduction command (run from the repository root after generating page.png):

```powershell
& 'C:\Program Files\WinRAR\Rar.exe' a -ma5 -m0 -s -hppublic-mimageviewer-rar-test -v30000b -ep -idq testdata/archives/rar-header-encrypted-multipart-legacy-cache/header-encrypted.rar target/A-1355-encrypted-fixture-input/page.png
```

The PNG uses deterministic xorshift32 bytes with initial state `0x1355`,
shifts `(13, 17, 5)` and 32-bit masking after left shifts. Each of 128 rows
has PNG filter byte zero followed by 384 RGB bytes from the low byte of each
successive state. IHDR is `(128, 128, 8, 2, 0, 0, 0)`; IDAT is Python
`zlib.compress` of those rows; the file contains only IHDR, IDAT and IEND.
The PNG is 49,363 bytes. Store compression ensures deterministic split
coverage independent of compressor heuristics. RAR headers are encrypted
with `-hp`, and volume sizes are 30,000 and 20,302 bytes.

Tests copy both volumes to disposable data directories and exercise the
real native UnRAR scanner/converter. WinRAR is needed only to regenerate
the fixture, not to run tests.

Native UnRAR cannot establish the volume kind from these encrypted headers
without the password. Opens do not infer the first volume from the filename.
The first-volume test enters a real PasswordRequired phase, retries with the
public password, converts, and adopts the first-volume cache successfully.
The no-cache later-volume test also enters the real password phase; retry at
the clicked input yields no image or nested-archive entries and must show the
Japanese guidance to open the first file of a multipart RAR, without inventing
a first filename. No synthetic password phase or summary is used for these tests.

Plaintext multipart control tests separately reject a header-confirmed later
volume before Direct/cache/conversion selection and name the first filename
resolved from the real header. Allowed first-volume opens retain cache sharing,
explicit sibling conversion, deletion/reconversion, and reading-resume coverage.
