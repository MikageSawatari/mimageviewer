#!/usr/bin/env python3
"""Generate a folder of deliberately distinguishable PDFs for navigation tests.

Each document gets its own page size and fill colour, and every page prints which document and
page it is. Two properties matter for the automated checks:

* **Distinct page geometry per document.** The perf trace records the decoded width and height of
  every page, so a page belonging to document A shown while document B is open is visible in the
  trace as a size that cannot belong to B - no pixel comparison needed.
* **Distinct colour and text per page.** That is for the human looking at a failure, and for
  screenshots.

Pure standard library on purpose: unlike a video fixture, a valid PDF is small enough to write by
hand, so the scenario needs nothing fetched or pre-built.
"""

from __future__ import annotations

import argparse
from pathlib import Path

# Page sizes are all clearly different aspect ratios so a mismatch is obvious both on screen and
# in the trace. Widths stay in a plausible scanned-page range so layout behaves normally.
SIZES = [
    (612, 792),
    (595, 842),
    (540, 900),
    (720, 720),
    (480, 960),
    (700, 850),
    (560, 800),
    (640, 880),
]

# Distinct, high-contrast fills. Index matches SIZES.
COLOURS = [
    (0.90, 0.20, 0.20),
    (0.20, 0.55, 0.90),
    (0.20, 0.70, 0.35),
    (0.95, 0.75, 0.15),
    (0.65, 0.30, 0.85),
    (0.15, 0.75, 0.75),
    (0.95, 0.50, 0.20),
    (0.45, 0.45, 0.50),
]


def content_stream(doc: int, page: int, width: int, height: int) -> bytes:
    r, g, b = COLOURS[doc % len(COLOURS)]
    # Fill the page, then a white band with large text naming the document and page.
    band_h = height * 0.22
    band_y = height * 0.5 - band_h * 0.5
    text = f"DOC {doc:02d} PAGE {page:02d}"
    size_text = f"{width} x {height}"
    ops = f"""{r:.3f} {g:.3f} {b:.3f} rg
0 0 {width} {height} re f
1 1 1 rg
0 {band_y:.1f} {width} {band_h:.1f} re f
0 0 0 rg
BT /F1 {height * 0.075:.1f} Tf {width * 0.08:.1f} {height * 0.52:.1f} Td ({text}) Tj ET
BT /F1 {height * 0.035:.1f} Tf {width * 0.08:.1f} {height * 0.46:.1f} Td ({size_text}) Tj ET
"""
    return ops.encode("ascii")


def build_pdf(doc: int, pages: int, width: int, height: int) -> bytes:
    objects: list[bytes] = []

    def add(body: bytes) -> int:
        objects.append(body)
        return len(objects)

    # Reserve object 1 for the catalog and 2 for the page tree so the ids are predictable.
    objects.append(b"")  # 1: catalog, filled in below
    objects.append(b"")  # 2: pages, filled in below
    font_id = add(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")

    kids: list[int] = []
    for page in range(1, pages + 1):
        stream = content_stream(doc, page, width, height)
        content_id = add(
            b"<< /Length " + str(len(stream)).encode("ascii") + b" >>\nstream\n" + stream + b"endstream"
        )
        page_id = add(
            f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] "
            f"/Resources << /Font << /F1 {font_id} 0 R >> >> /Contents {content_id} 0 R >>".encode("ascii")
        )
        kids.append(page_id)

    kid_refs = " ".join(f"{k} 0 R" for k in kids)
    objects[1] = f"<< /Type /Pages /Kids [{kid_refs}] /Count {len(kids)} >>".encode("ascii")
    objects[0] = b"<< /Type /Catalog /Pages 2 0 R >>"

    out = bytearray(b"%PDF-1.4\n")
    offsets = [0] * (len(objects) + 1)
    for i, body in enumerate(objects, start=1):
        offsets[i] = len(out)
        out += f"{i} 0 obj\n".encode("ascii") + body + b"\nendobj\n"

    xref_at = len(out)
    out += f"xref\n0 {len(objects) + 1}\n".encode("ascii")
    out += b"0000000000 65535 f \n"
    for i in range(1, len(objects) + 1):
        out += f"{offsets[i]:010d} 00000 n \n".encode("ascii")
    out += (
        f"trailer\n<< /Size {len(objects) + 1} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n"
    ).encode("ascii")
    return bytes(out)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    parser.add_argument("--docs", type=int, default=8)
    parser.add_argument("--pages", type=int, default=6)
    args = parser.parse_args()
    if args.docs < 2:
        parser.error("--docs must be at least 2 for navigation between documents")
    if args.pages < 2:
        parser.error("--pages must be at least 2 so a page turn stays inside one document")

    args.output.mkdir(parents=True, exist_ok=True)
    for existing in args.output.glob("*.pdf"):
        existing.unlink()

    for doc in range(args.docs):
        width, height = SIZES[doc % len(SIZES)]
        path = args.output / f"doc{doc:02d}.pdf"
        path.write_bytes(build_pdf(doc, args.pages, width, height))
        print(f"{path}  {args.pages} pages  {width}x{height}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
