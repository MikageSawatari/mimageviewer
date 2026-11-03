r"""Build the imatrix calibration text for the distributed VLM model (plan §21.7.1).

Uses only mIV's own Japanese manual (htdocs/mimageviewer/manual/*.html), so the
calibration data has no third-party license. Strips script/style/nav/header/footer,
tags, and lines shorter than 8 characters.

    python make_calibration.py --manual <repo>\htdocs\mimageviewer\manual --out calibration-miv-manual-ja.txt
"""

import argparse
import glob
import html
import os
import pathlib
import re


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--manual", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    out = []
    files = sorted(glob.glob(os.path.join(a.manual, "*.html")))
    for p in files:
        s = open(p, encoding="utf-8").read()
        s = re.sub(r"(?is)<(script|style|nav|header|footer)\b.*?</\1>", " ", s)
        s = re.sub(r"(?s)<[^>]+>", "\n", s)
        s = html.unescape(s)
        lines = [line.strip() for line in s.splitlines()]
        out.append("\n".join(line for line in lines if len(line) >= 8))
    text = "\n\n".join(out)
    pathlib.Path(a.out).write_text(text, encoding="utf-8")
    print(len(files), "files", len(text), "chars")


if __name__ == "__main__":
    main()
