#!/usr/bin/env python3
"""Generate synthetic manga pages for view-trim compatibility checks.

The outputs are intentionally simple black-and-white PNG pages with large,
controlled margins. They are for manual verification in mImageViewer and older
comic/image viewers, not for automated tests.
"""

from __future__ import annotations

import shutil
import struct
import zipfile
import zlib
from pathlib import Path


REPO = Path(__file__).resolve().parents[1]
OUT = REPO / "samples" / "view-trim-manga"
PAGES = OUT / "pages"
ZIP_PATH = OUT / "view-trim-manga-sample.zip"
W, H = 1000, 1400

WHITE = (255, 255, 255)
INK = (18, 18, 18)
GRAY = (170, 170, 170)
LIGHT = (236, 236, 236)
BLACK = (0, 0, 0)


FONT = {
    " ": ["000", "000", "000", "000", "000", "000", "000"],
    "-": ["000", "000", "000", "111", "000", "000", "000"],
    "0": ["111", "101", "101", "101", "101", "101", "111"],
    "1": ["010", "110", "010", "010", "010", "010", "111"],
    "2": ["111", "001", "001", "111", "100", "100", "111"],
    "3": ["111", "001", "001", "111", "001", "001", "111"],
    "4": ["101", "101", "101", "111", "001", "001", "001"],
    "5": ["111", "100", "100", "111", "001", "001", "111"],
    "6": ["111", "100", "100", "111", "101", "101", "111"],
    "7": ["111", "001", "001", "010", "010", "100", "100"],
    "8": ["111", "101", "101", "111", "101", "101", "111"],
    "9": ["111", "101", "101", "111", "001", "001", "111"],
    "A": ["111", "101", "101", "111", "101", "101", "101"],
    "B": ["110", "101", "101", "110", "101", "101", "110"],
    "C": ["111", "100", "100", "100", "100", "100", "111"],
    "D": ["110", "101", "101", "101", "101", "101", "110"],
    "E": ["111", "100", "100", "111", "100", "100", "111"],
    "F": ["111", "100", "100", "111", "100", "100", "100"],
    "G": ["111", "100", "100", "101", "101", "101", "111"],
    "H": ["101", "101", "101", "111", "101", "101", "101"],
    "I": ["111", "010", "010", "010", "010", "010", "111"],
    "K": ["101", "101", "110", "100", "110", "101", "101"],
    "L": ["100", "100", "100", "100", "100", "100", "111"],
    "M": ["101", "111", "111", "101", "101", "101", "101"],
    "N": ["101", "111", "111", "111", "101", "101", "101"],
    "O": ["111", "101", "101", "101", "101", "101", "111"],
    "P": ["111", "101", "101", "111", "100", "100", "100"],
    "R": ["110", "101", "101", "110", "110", "101", "101"],
    "S": ["111", "100", "100", "111", "001", "001", "111"],
    "T": ["111", "010", "010", "010", "010", "010", "010"],
    "U": ["101", "101", "101", "101", "101", "101", "111"],
    "V": ["101", "101", "101", "101", "101", "101", "010"],
    "W": ["101", "101", "101", "101", "111", "111", "101"],
    "Y": ["101", "101", "101", "010", "010", "010", "010"],
}


class Canvas:
    def __init__(self, width: int, height: int, color: tuple[int, int, int]):
        self.width = width
        self.height = height
        self.pixels = bytearray(color * (width * height))

    def set_px(self, x: int, y: int, color: tuple[int, int, int]) -> None:
        if 0 <= x < self.width and 0 <= y < self.height:
            i = (y * self.width + x) * 3
            self.pixels[i : i + 3] = bytes(color)

    def rect(self, x0: int, y0: int, x1: int, y1: int, color: tuple[int, int, int]) -> None:
        x0, x1 = sorted((max(0, x0), min(self.width, x1)))
        y0, y1 = sorted((max(0, y0), min(self.height, y1)))
        row = bytes(color) * (x1 - x0)
        for y in range(y0, y1):
            i = (y * self.width + x0) * 3
            self.pixels[i : i + len(row)] = row

    def line(
        self,
        x0: int,
        y0: int,
        x1: int,
        y1: int,
        color: tuple[int, int, int],
        width: int = 1,
    ) -> None:
        dx = abs(x1 - x0)
        dy = -abs(y1 - y0)
        sx = 1 if x0 < x1 else -1
        sy = 1 if y0 < y1 else -1
        err = dx + dy
        half = max(0, width // 2)
        while True:
            self.rect(x0 - half, y0 - half, x0 + half + 1, y0 + half + 1, color)
            if x0 == x1 and y0 == y1:
                break
            e2 = 2 * err
            if e2 >= dy:
                err += dy
                x0 += sx
            if e2 <= dx:
                err += dx
                y0 += sy

    def outline_rect(
        self, x0: int, y0: int, x1: int, y1: int, color: tuple[int, int, int], width: int = 3
    ) -> None:
        self.rect(x0, y0, x1, y0 + width, color)
        self.rect(x0, y1 - width, x1, y1, color)
        self.rect(x0, y0, x0 + width, y1, color)
        self.rect(x1 - width, y0, x1, y1, color)

    def ellipse(
        self,
        cx: int,
        cy: int,
        rx: int,
        ry: int,
        outline: tuple[int, int, int],
        fill: tuple[int, int, int] | None = None,
        width: int = 3,
    ) -> None:
        outer = 1.0
        inner = max(0.0, 1.0 - width / max(rx, ry))
        for y in range(cy - ry, cy + ry + 1):
            for x in range(cx - rx, cx + rx + 1):
                v = ((x - cx) * (x - cx)) / (rx * rx) + ((y - cy) * (y - cy)) / (ry * ry)
                if fill and v <= outer:
                    self.set_px(x, y, fill)
                if inner <= v <= outer:
                    self.set_px(x, y, outline)

    def text(self, x: int, y: int, msg: str, scale: int, color: tuple[int, int, int]) -> None:
        cx = x
        for ch in msg.upper():
            glyph = FONT.get(ch, FONT[" "])
            for gy, row in enumerate(glyph):
                for gx, bit in enumerate(row):
                    if bit == "1":
                        self.rect(cx + gx * scale, y + gy * scale, cx + (gx + 1) * scale, y + (gy + 1) * scale, color)
            cx += 4 * scale

    def save_png(self, path: Path) -> None:
        raw = bytearray()
        stride = self.width * 3
        for y in range(self.height):
            raw.append(0)
            start = y * stride
            raw.extend(self.pixels[start : start + stride])
        compressed = zlib.compress(bytes(raw), 9)

        def chunk(name: bytes, data: bytes) -> bytes:
            crc = zlib.crc32(name + data) & 0xFFFFFFFF
            return struct.pack(">I", len(data)) + name + data + struct.pack(">I", crc)

        path.write_bytes(
            b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", self.width, self.height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", compressed)
            + chunk(b"IEND", b"")
        )


def panel_grid(c: Canvas, x0: int, y0: int, x1: int, y1: int, side: str, label: str) -> None:
    c.outline_rect(x0, y0, x1, y1, INK, 6)
    mid_y = y0 + (y1 - y0) // 2
    mid_x = x0 + (x1 - x0) // 2
    c.line(x0, mid_y, x1, mid_y, INK, 5)
    c.line(mid_x, mid_y, mid_x, y1, INK, 5)

    # Wide top panel: horizon line continues at the same height across a spread.
    c.line(x0 + 22, y0 + 175, x1 - 22, y0 + 175, INK, 4)
    for i in range(9):
        sx = x0 + 40 + i * ((x1 - x0 - 80) // 8)
        c.line(sx, y0 + 175, max(x0 + 24, sx - 80), y0 + 50, GRAY, 2)

    # Big face / figure near the gutter edge, making page pairing obvious.
    if side == "right":
        face_x = x0 + 140
    else:
        face_x = x1 - 140
    c.ellipse(face_x, y0 + 245, 70, 92, INK, None, 5)
    c.line(face_x - 35, y0 + 235, face_x - 8, y0 + 222, INK, 4)
    c.line(face_x + 35, y0 + 235, face_x + 8, y0 + 222, INK, 4)
    c.line(face_x - 28, y0 + 275, face_x + 28, y0 + 275, INK, 3)
    c.line(face_x, y0 + 337, face_x, y0 + 430, INK, 8)
    c.line(face_x, y0 + 370, face_x - 70, y0 + 455, INK, 6)
    c.line(face_x, y0 + 370, face_x + 70, y0 + 455, INK, 6)

    # Lower panels: speech bubble and speed lines.
    bubble_x = x0 + max(130, (mid_x - x0) // 2)
    c.ellipse(bubble_x, mid_y + 150, 110, 65, INK, WHITE, 4)
    c.line(bubble_x + 50, mid_y + 205, bubble_x + 95, mid_y + 270, INK, 4)
    c.text(bubble_x - 58, mid_y + 122, "TRIM", 8, INK)
    c.text(bubble_x - 58, mid_y + 158, "TEST", 8, INK)
    c.text(x0 + 48, y1 - 118, label, 5, INK)

    for i in range(14):
        c.line(mid_x + 35, mid_y + 35 + i * 36, x1 - 35, mid_y + 5 + i * 18, INK, 3)


def draw_page(page_no: int, side: str, black_margin: bool, margins: tuple[int, int, int, int], label: str) -> Path:
    c = Canvas(W, H, BLACK if black_margin else WHITE)
    if black_margin:
        paper = (118, 96, W - 118, H - 106)
        c.rect(*paper, WHITE)
        c.outline_rect(*paper, LIGHT, 4)
        base_left, base_top, base_right, base_bottom = paper
    else:
        base_left, base_top, base_right, base_bottom = 0, 0, W, H

    ml, mt, mr, mb = margins
    x0 = base_left + ml
    y0 = base_top + mt
    x1 = base_right - mr
    y1 = base_bottom - mb
    panel_grid(c, x0, y0, x1, y1, side, label)

    # A small dark registration strip inside the art area helps detect whether a
    # viewer crops only the outer blank margin or also clips valid content.
    if side == "right":
        c.rect(x0 + 20, y1 - 52, x0 + 150, y1 - 32, INK)
    else:
        c.rect(x1 - 150, y1 - 52, x1 - 20, y1 - 32, INK)

    name = f"{page_no:03d}_{label.lower().replace(' ', '_')}.png"
    path = PAGES / name
    c.save_png(path)
    return path


def write_readme() -> None:
    (OUT / "README.md").write_text(
        """# view-trim manga sample

表示トリム / 余白カット / クリッピング機能を実機確認するための合成漫画サンプルです。

- `pages/` は通常フォルダとして開く確認用です。
- `view-trim-manga-sample.zip` は ZIP / 漫画アーカイブとして開く確認用です。
- 画像はすべて合成データです。外側に大きな白余白または黒余白を入れ、中央に漫画風の線画を置いています。

## 確認ポイント

1. フォルダと ZIP の両方で同じように開けるか。
2. 白い外側余白を自動で消せるか。
3. 黒い外側余白を自動で消せるか。
4. 見開き表示で、左右ページの上段パネルの水平線が自然につながるか。
5. 余白トリムを有効にしても、ページ内のコマ線や下部の黒い帯が欠けないか。
6. 本全体の設定とページごとの設定を分けて保存できるか。

mImageViewer では左パネルの「表示トリム」から、自動余白カット、本全体、このページだけ、見開き左右別の調整を確認できます。
""",
        encoding="utf-8",
    )


def main() -> None:
    if PAGES.exists():
        shutil.rmtree(PAGES)
    PAGES.mkdir(parents=True, exist_ok=True)
    OUT.mkdir(parents=True, exist_ok=True)

    pages = [
        draw_page(1, "right", False, (240, 210, 130, 190), "WHITE COVER"),
        draw_page(2, "right", False, (90, 155, 210, 150), "RIGHT WHITE"),
        draw_page(3, "left", False, (210, 155, 90, 150), "LEFT WHITE"),
        draw_page(4, "right", True, (70, 92, 155, 96), "RIGHT BLACK"),
        draw_page(5, "left", True, (155, 92, 70, 96), "LEFT BLACK"),
        draw_page(6, "left", False, (185, 270, 70, 95), "UNEQUAL"),
    ]

    if ZIP_PATH.exists():
        ZIP_PATH.unlink()
    with zipfile.ZipFile(ZIP_PATH, "w", zipfile.ZIP_DEFLATED) as zf:
        for page in pages:
            zf.write(page, page.name)

    write_readme()
    print(f"wrote {len(pages)} pages to {PAGES}")
    print(f"wrote {ZIP_PATH}")


if __name__ == "__main__":
    main()
