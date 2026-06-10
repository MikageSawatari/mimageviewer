#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""ネスト ZIP ツリーナビ (v1.3.0) の実機確認用テスト ZIP を生成する。

docs/nested-zip-tree-plan.md の確認項目を 1 つずつ踏めるように、構造を作り分けた
ZIP を `dist/ziptest/` に出力する。各ページ画像はラベル付き (本名・ページ番号・
表紙バナー・構造の説明) なので、見開きペアリングや代表サムネを目視で検証できる。

生成物:
  dist/ziptest/nested_tree_test.zip   メインのネスト ZIP (多構造)
  dist/ziptest/single_book_wrapper.zip 単一ラッパー (開いた瞬間に自動降下)
  dist/ziptest/simple_flat.zip        フラット (退行確認のベースライン)
  dist/ziptest/big_nested.zip         大量エントリ (UI が固まらないか、--big 指定時)

使い方:
  python scripts/make_nested_zip_test.py          # 主要 3 つを生成
  python scripts/make_nested_zip_test.py --big     # + big_nested.zip も生成
"""

import io
import os
import sys
import zipfile

from PIL import Image, ImageDraw, ImageFont

OUT_DIR = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "dist", "ziptest")

W, H = 720, 1040  # 縦長ページ (見開きでペアになりやすい)

# 本ごとの背景色 (一目で本を見分けられるように鮮やかに分ける)
PALETTE = {
    "A": (210, 70, 70),
    "B": (70, 140, 210),
    "C": (90, 180, 110),
    "X": (200, 140, 50),
    "Y": (150, 90, 190),
    "W": (60, 170, 175),
    "D1": (200, 90, 150),
    "D2": (120, 120, 60),
    "M": (90, 110, 200),
    "MS": (180, 110, 80),
    "S": (110, 110, 120),
    "F": (80, 150, 90),
}


def _font(size):
    for path in (
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\arial.ttf",
    ):
        try:
            return ImageFont.truetype(path, size)
        except OSError:
            continue
    try:
        return ImageFont.load_default(size)
    except TypeError:
        return ImageFont.load_default()


def _wrap(draw, text, font, max_w):
    out = []
    for raw in text.split("\n"):
        line = ""
        for ch in raw:
            if draw.textlength(line + ch, font=font) <= max_w:
                line += ch
            else:
                out.append(line)
                line = ch
        out.append(line)
    return out


def page_png(book, page_num, total, *, cover_note=None):
    """1 ページの PNG バイト列を返す。"""
    color = PALETTE.get(book, (100, 100, 100))
    img = Image.new("RGB", (W, H), color)
    d = ImageDraw.Draw(img)
    # 上部: 本名
    d.text((W // 2, 70), f"BOOK {book}", font=_font(64), fill=(255, 255, 255), anchor="mm")
    # 中央: 巨大なページ番号
    d.text((W // 2, H // 2 - 40), f"{page_num}", font=_font(360), fill=(255, 255, 255), anchor="mm")
    d.text((W // 2, H // 2 + 170), f"page {page_num} / {total}", font=_font(44),
           fill=(255, 255, 255), anchor="mm")
    if page_num == 1:
        # 表紙バナー (代表サムネ + cover 単独表示の確認用)
        d.rectangle([(0, H // 2 - 160), (W, H // 2 - 90)], fill=(0, 0, 0))
        d.text((W // 2, H // 2 - 125), "★ COVER ★", font=_font(48), fill=(255, 230, 60), anchor="mm")
    # 下部: 構造の説明 (cover のみ。自己説明的なテスト用)
    if cover_note:
        box_top = H - 300
        d.rectangle([(20, box_top), (W - 20, H - 20)], fill=(0, 0, 0))
        f = _font(26)
        y = box_top + 18
        for line in _wrap(d, cover_note, f, W - 80):
            d.text((40, y), line, font=f, fill=(255, 255, 255))
            y += 34
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return buf.getvalue()


def small_png(book, page_num):
    """big zip 用の軽量ページ (256x384、ソリッド + 番号)。"""
    color = PALETTE.get(book, (100, 100, 100))
    img = Image.new("RGB", (256, 384), color)
    d = ImageDraw.Draw(img)
    d.text((128, 192), f"{book}\n{page_num}", font=_font(56), fill=(255, 255, 255), anchor="mm", align="center")
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return buf.getvalue()


def book_pages(book, count, *, padded=True, cover_note=None, prefix=""):
    """1 冊分の (entry_name, png_bytes) リストを返す。

    padded=False のときファイル名を p1.png..p{n}.png にして、番号順ソートと
    ファイル名順ソートで並びが変わる (numeric sort の検証用)。
    """
    out = []
    for i in range(1, count + 1):
        if padded:
            name = f"{prefix}{book.lower()}_{i:02d}.png"
        else:
            name = f"{prefix}p{i}.png"
        note = cover_note if i == 1 else None
        out.append((name, page_png(book, i, count, cover_note=note)))
    return out


def make_inner_zip(entries):
    """(name, bytes) のリストから内側 ZIP のバイト列を作る (STORED)。"""
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_STORED) as zf:
        for name, data in entries:
            zf.writestr(name, data)
    return buf.getvalue()


def write_zip(path, file_entries):
    """file_entries = [(arcname, bytes), ...] を 1 つの ZIP に書き出す。"""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_STORED) as zf:
        for name, data in file_entries:
            zf.writestr(name, data)
    print(f"  wrote {path}  ({len(file_entries)} top-level entries)")


def build_nested_tree_test(path):
    e = []  # outer entries: (arcname, bytes)

    # ルート直下の loose 画像 (コンテナと画像の混在 + コンテナ先頭の並び確認)
    e.append(("root_a.png", page_png("S", 1, 2, cover_note="ルート直下の loose 画像 1/2。\nコンテナ (フォルダ型セル) が先、画像が後に並ぶか確認。")))
    e.append(("root_b.png", page_png("S", 2, 2)))

    # 01_inner_archives/ : 内側アーカイブ (.zip / .cbz) のサブフォルダ
    chA = make_inner_zip(book_pages(
        "A", 11, padded=False,
        cover_note="01_inner_archives/chapterA.zip\n11 頁 (奇数) / ファイル名 p1..p11 (非ゼロ詰め)\nTEST: 番号順ソート (p2 が p10 より前) + 奇数頁の見開きリセット\nバッジ=ZIP"))
    e.append(("01_inner_archives/chapterA.zip", chA))
    chB = make_inner_zip(book_pages(
        "B", 4,
        cover_note="01_inner_archives/chapterB.cbz\n4 頁 (偶数) / .cbz 拡張子\nTEST: .cbz も内側アーカイブとして ZIP バッジで入れる"))
    e.append(("01_inner_archives/chapterB.cbz", chB))
    chC = make_inner_zip(book_pages(
        "C", 3,
        cover_note="01_inner_archives/chapterC.zip\n3 頁 (奇数)\nTEST: 各本で表紙が単独表示 / 本またぎ連結が起きない"))
    e.append(("01_inner_archives/chapterC.zip", chC))

    # 02_plain_subfolders/ : ただのサブフォルダ (アーカイブでない) の本
    for name, png in book_pages(
        "X", 6, prefix="vol_X/",
        cover_note="02_plain_subfolders/vol_X/\n6 頁 / ただのサブフォルダ (非アーカイブ)\nTEST: フォルダ型バッジ (ZIP バッジではない) で入れる"):
        e.append((f"02_plain_subfolders/{name}", png))
    for name, png in book_pages(
        "Y", 2, prefix="vol_Y/",
        cover_note="02_plain_subfolders/vol_Y/\n2 頁\nTEST: 短い本でも cover 単独 + 2 頁目"):
        e.append((f"02_plain_subfolders/{name}", png))

    # 03_single_wrapper/ : 子ディレクトリ 1 個・画像 0 枚 → 自動降下 (D1)
    for name, png in book_pages(
        "W", 5, prefix="only/",
        cover_note="03_single_wrapper/only/\n5 頁\nTEST(D1): 03_single_wrapper を開くと中間の 'only' を自動スキップして\nここ (頁) に直接降りる"):
        e.append((f"03_single_wrapper/{name}", png))

    # 04_deep_nest/ : 深いネスト。inner1.zip(直下頁) と inner2.zip(さらに subfolder)
    d1 = make_inner_zip(book_pages(
        "D1", 3,
        cover_note="04_deep_nest/inner1.zip\n3 頁 (zip 直下)\nTEST: root>04_deep_nest>{inner1,inner2} の 3 段ナビ"))
    e.append(("04_deep_nest/inner1.zip", d1))
    d2 = make_inner_zip(book_pages(
        "D2", 3, prefix="deeper/",
        cover_note="04_deep_nest/inner2.zip/deeper/\n3 頁\nTEST(D1 deep): inner2.zip を開くと中の 'deeper' を自動スキップ"))
    e.append(("04_deep_nest/inner2.zip", d2))

    # 05_mixed/ : 同一階層に loose 画像 + サブ本 (代表サムネ=loose 優先)
    e.append(("05_mixed/m_01.png", page_png(
        "M", 1, 2,
        cover_note="05_mixed/ (混在階層)\n直下 loose 画像 m_01/m_02 + サブ本 extra.zip\nTEST: 05_mixed の代表サムネは直下画像 m_01 (サブ本でなく)\n表示順は extra.zip (コンテナ) が先、m_01/m_02 が後")))
    e.append(("05_mixed/m_02.png", page_png("M", 2, 2)))
    ex = make_inner_zip(book_pages(
        "MS", 3,
        cover_note="05_mixed/extra.zip\n3 頁\nTEST: 混在階層内のサブ本に入れる"))
    e.append(("05_mixed/extra.zip", ex))

    write_zip(path, e)


def build_single_book_wrapper(path):
    # ZIP 直下が単一フォルダ 1 個・画像 0 枚 → 開いた瞬間に自動降下して頁に着地
    e = []
    for name, png in book_pages(
        "F", 8, prefix="the_only_folder/",
        cover_note="single_book_wrapper.zip\nthe_only_folder/ の中に 8 頁\nTEST(D1 root): この ZIP を開くと 'the_only_folder' を飛ばして\nいきなり頁一覧 (ルート崩し)。Backspace で即 ZIP を抜ける"):
        e.append((name, png))
    write_zip(path, e)


def build_simple_flat(path):
    # ルート直下に画像のみ。ZipDir セルは一切出ない (退行ベースライン)
    e = []
    for name, png in book_pages(
        "F", 8,
        cover_note="simple_flat.zip\nルート直下に 8 頁のみ (サブフォルダ無し)\nTEST(退行): 従来通りフラット表示。ZipDir セルは出ない。\n見開き/連続読みが従来通り"):
        e.append((name, png))
    write_zip(path, e)


def build_big_nested(path):
    # 大量エントリ: 6 本 × 150 頁 = 900 頁。UI が固まらないか確認用 (軽量画像)。
    e = []
    for b in range(1, 7):
        book = f"BIG{b}"
        PALETTE[book] = (40 + b * 30, 90, 200 - b * 20)
        entries = [(f"p{i:03d}.png", small_png(book, i)) for i in range(1, 151)]
        e.append((f"book_{b:02d}.zip", make_inner_zip(entries)))
    write_zip(path, e)


def main():
    big = "--big" in sys.argv[1:]
    os.makedirs(OUT_DIR, exist_ok=True)
    print(f"Generating test ZIPs into {OUT_DIR}")
    build_nested_tree_test(os.path.join(OUT_DIR, "nested_tree_test.zip"))
    build_single_book_wrapper(os.path.join(OUT_DIR, "single_book_wrapper.zip"))
    build_simple_flat(os.path.join(OUT_DIR, "simple_flat.zip"))
    if big:
        build_big_nested(os.path.join(OUT_DIR, "big_nested.zip"))
    else:
        print("  (skipped big_nested.zip; pass --big to generate the 900-page stress ZIP)")
    print("Done.")


if __name__ == "__main__":
    main()
