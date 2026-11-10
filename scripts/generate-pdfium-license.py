#!/usr/bin/env python3
"""Generate PDFIUM-LICENSE.txt from the bundled PDFium distribution.

Usage (from the repository root, including PowerShell):
    python scripts/generate-pdfium-license.py
    python scripts/generate-pdfium-license.py --check

This is the single format definition, also used by the About regression test.
Output is UTF-8 without BOM, with LF line endings. Source whitespace, including
trailing blank lines, is preserved; a missing final newline is supplied before
the next section. Only freetype.txt is decoded as Latin-1; all other inputs
must be UTF-8. Check mode ignores line-ending differences, but no other edits.
"""

import argparse
from pathlib import Path
import re
import sys

REPO = Path(__file__).resolve().parent.parent
REGENERATE = "python scripts/generate-pdfium-license.py"


def normalize_newlines(text):
    return text.replace("\r\n", "\n").replace("\r", "\n")


def generate(vendor):
    version_fields = dict(
        line.split("=", 1)
        for line in (vendor / "VERSION").read_text(encoding="utf-8").splitlines()
        if line
    )
    version = ".".join(version_fields[key] for key in ("MAJOR", "MINOR", "BUILD", "PATCH"))
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+", version):
        raise ValueError("Invalid vendor/pdfium/VERSION")

    primary = vendor / "licenses/pdfium.txt"
    if not primary.is_file():
        raise FileNotFoundError(primary)
    sources = [vendor / "LICENSE", primary]
    sources += sorted(
        (path for path in (vendor / "licenses").rglob("*") if path.is_file() and path != primary),
        key=lambda path: path.relative_to(vendor).as_posix(),
    )
    text = (
        f"Licenses and copyright notices for PDFium {version} bundled with mImageViewer\n"
        "Distribution: bblanchon/pdfium-binaries, pdfium-win-x64.tgz\n"
        "Includes PDFium and its internal components.\n"
        "This software is based in part on the work of the FreeType Team.\n\n"
    )
    separator = "=" * 78
    for path in sources:
        relative = path.relative_to(vendor).as_posix()
        encoding = "iso-8859-1" if relative == "licenses/freetype.txt" else "utf-8"
        body = normalize_newlines(path.read_bytes().decode(encoding))
        text += f"{separator}\n{relative}\n{separator}\n\n{body}"
        if not body.endswith("\n"):
            text += "\n"
        text += "\n"
    return text


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="check without rewriting")
    args = parser.parse_args()
    output = REPO / "PDFIUM-LICENSE.txt"
    try:
        expected = generate(REPO / "vendor/pdfium")
        if args.check:
            actual = normalize_newlines(output.read_bytes().decode("utf-8"))
            if actual != expected:
                raise ValueError(
                    "PDFIUM-LICENSE.txt differs from vendor/pdfium "
                    "(version, section names/order, or full license text)"
                )
            print("PDFIUM-LICENSE.txt matches vendor/pdfium (all notices and VERSION).")
        else:
            output.write_bytes(expected.encode("utf-8"))
            print("Generated PDFIUM-LICENSE.txt.")
    except (OSError, UnicodeError, ValueError, KeyError) as error:
        print(f"{error}\nAfter setting up vendor/pdfium, run: {REGENERATE}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
