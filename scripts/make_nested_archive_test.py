#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""入れ子アーカイブ展開 (v1.3.0、RAR/7z/LZH in ZIP / ZIP in RAR 等) の実機確認用
テストアーカイブを生成する。

`make_nested_zip_test.py` (純 ZIP ツリー用) の姉妹スクリプト。ページ画像の生成は
同スクリプトから import して使う (自己説明的なラベル付きページ)。

生成物 (dist/ziptest/):
  foreign_in_zip.zip   ZIP の中に 7z (+ WinRAR があれば rar) が入れ子。
                       開くと「ZIP 内のアーカイブを展開」ダイアログが出るのを確認
  nested_7z_test.7z    7z の中に zip / 7z / サブフォルダが入れ子 (変換の再帰展開)
  rar_in_zip.zip       ZIP の中に rar           (要 WinRAR)
  zip_in_rar.rar       rar の中に zip           (要 WinRAR)
  nested_rar_test.rar  rar の中に rar           (要 WinRAR)

使い方:
  python scripts/make_nested_archive_test.py

前提ツール:
  - 7-Zip (7z.exe)   : 7z 系サンプルの生成に必須
  - WinRAR (Rar.exe) : rar 系サンプルの生成に必要。無ければ自動でスキップして
                       案内を表示する (7z 系だけでも展開コードの大半は検証できる)
"""

import io
import os
import shutil
import subprocess
import sys
import tempfile
import zipfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from make_nested_zip_test import PALETTE, book_pages, make_inner_zip, page_png  # noqa: E402

OUT_DIR = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "dist", "ziptest"
)

PALETTE.update(
    {
        "Z7": (40, 130, 160),   # 7z の中身
        "ZR": (170, 60, 60),    # rar の中身
        "ZZ": (70, 140, 70),    # 対照群の普通の zip
        "NR": (140, 80, 160),   # rar in rar
    }
)


def find_7z():
    p = shutil.which("7z")
    if p:
        return p
    cand = r"C:\Program Files\7-Zip\7z.exe"
    return cand if os.path.exists(cand) else None


def find_rar():
    for name in ("rar", "Rar"):
        p = shutil.which(name)
        if p:
            return p
    for cand in (
        r"C:\Program Files\WinRAR\Rar.exe",
        r"C:\Program Files (x86)\WinRAR\Rar.exe",
    ):
        if os.path.exists(cand):
            return cand
    return None


def write_book_to_dir(staging, book, count, *, subdir="", cover_note=None):
    """staging/subdir に 1 冊分のページ PNG を実ファイルとして書く。"""
    target = os.path.join(staging, subdir) if subdir else staging
    os.makedirs(target, exist_ok=True)
    for name, data in book_pages(book, count, cover_note=cover_note):
        with open(os.path.join(target, name), "wb") as f:
            f.write(data)


def pack_7z(sevenz, staging, out_path):
    if os.path.exists(out_path):
        os.remove(out_path)
    subprocess.run(
        [sevenz, "a", "-t7z", "-mx=1", "-bso0", "-bsp0", os.path.abspath(out_path), "*"],
        cwd=staging,
        check=True,
    )


def pack_rar(rar, staging, out_path):
    if os.path.exists(out_path):
        os.remove(out_path)
    # -ep1: 格納パスから staging プレフィックスを除く / -idq: quiet / -r: サブフォルダ込み
    subprocess.run(
        [rar, "a", "-ep1", "-idq", "-r", os.path.abspath(out_path), "*"],
        cwd=staging,
        check=True,
    )


def make_7z_bytes(sevenz, build_fn):
    """staging に build_fn で中身を作って 7z 圧縮し、バイト列を返す。"""
    with tempfile.TemporaryDirectory() as staging:
        build_fn(staging)
        out = os.path.join(staging + "_out", "t.7z")
        os.makedirs(os.path.dirname(out), exist_ok=True)
        pack_7z(sevenz, staging, out)
        with open(out, "rb") as f:
            data = f.read()
        shutil.rmtree(os.path.dirname(out), ignore_errors=True)
        return data


def make_rar_bytes(rar, build_fn):
    with tempfile.TemporaryDirectory() as staging:
        build_fn(staging)
        out = os.path.join(staging + "_out", "t.rar")
        os.makedirs(os.path.dirname(out), exist_ok=True)
        pack_rar(rar, staging, out)
        with open(out, "rb") as f:
            data = f.read()
        shutil.rmtree(os.path.dirname(out), ignore_errors=True)
        return data


def write_zip(path, file_entries):
    with zipfile.ZipFile(path, "w", zipfile.ZIP_STORED) as zf:
        for name, data in file_entries:
            zf.writestr(name, data)
    print(f"  wrote {path}  ({len(file_entries)} top-level entries)")


def build_foreign_in_zip(path, sevenz, rar):
    """ZIP > {直下画像, part1.7z, part2.zip, (part3.rar)}。
    開くと変換提案ダイアログが出て、変換後は 7z/rar の中身も本として見える。"""
    e = []
    e.append(
        (
            "cover_root.png",
            page_png(
                "S",
                1,
                1,
                cover_note=(
                    "foreign_in_zip.zip ルート直下画像\n"
                    "この ZIP には part1.7z (+ part3.rar) が入れ子。\n"
                    "TEST: 開くと『ZIP 内のアーカイブを展開』提案が出る。\n"
                    "キャンセル→7z/rar の本は見えないまま閲覧継続。\n"
                    "変換→part1.7z 等が本として現れる"
                ),
            ),
        )
    )

    def seven_inner(staging):
        write_book_to_dir(
            staging,
            "Z7",
            4,
            cover_note=(
                "foreign_in_zip.zip > part1.7z\n4 頁\n"
                "TEST: 変換後にこの本が ZIP バッジ付きで現れ、\n"
                "見開きが本単位でリセットされる"
            ),
        )
        # 7z の中にさらにサブフォルダ (アーカイブではない階層も保持されるか)
        write_book_to_dir(staging, "Z7", 2, subdir="bonus")

    e.append(("inner/part1.7z", make_7z_bytes(sevenz, seven_inner)))

    # 対照群: 普通のネスト ZIP (変換しなくても見える)
    e.append(
        (
            "inner/part2.zip",
            make_inner_zip(
                book_pages(
                    "ZZ",
                    3,
                    cover_note=(
                        "foreign_in_zip.zip > part2.zip (普通のネスト ZIP)\n"
                        "TEST: これは変換**前**から見える対照群。\n"
                        "変換後も同じように見え続ける"
                    ),
                )
            ),
        )
    )

    if rar:
        def rar_inner(staging):
            write_book_to_dir(
                staging,
                "ZR",
                3,
                cover_note=(
                    "foreign_in_zip.zip > part3.rar\n3 頁\n"
                    "TEST: 変換後に rar の中身が本として現れる"
                ),
            )

        e.append(("inner/part3.rar", make_rar_bytes(rar, rar_inner)))

    write_zip(path, e)


def build_nested_7z_test(path, sevenz):
    """7z > {直下画像, inside.zip, deep.7z, sub/}。外側が非 ZIP の再帰展開確認。"""

    def build(staging):
        # 直下画像
        for name, data in book_pages(
            "Z7",
            2,
            cover_note=(
                "nested_7z_test.7z 直下 2 頁\n"
                "TEST: 7z を開くと従来どおり変換ダイアログ。\n"
                "変換後、inside.zip / deep.7z / sub の 3 冊 +\n"
                "直下頁がツリーで見える (入れ子も展開済み)"
            ),
        ):
            with open(os.path.join(staging, name), "wb") as f:
                f.write(data)
        # 入れ子 zip
        inside = make_inner_zip(
            book_pages(
                "ZZ",
                3,
                cover_note="nested_7z_test.7z > inside.zip\n3 頁\nTEST: 7z 内の zip も展開される",
            )
        )
        with open(os.path.join(staging, "inside.zip"), "wb") as f:
            f.write(inside)
        # 入れ子 7z (同形式の入れ子)
        def deep(staging2):
            write_book_to_dir(
                staging2,
                "Z7",
                3,
                cover_note="nested_7z_test.7z > deep.7z\n3 頁\nTEST: 7z 内の 7z も展開される",
            )

        with open(os.path.join(staging, "deep.7z"), "wb") as f:
            f.write(make_7z_bytes(sevenz, deep))
        # ただのサブフォルダ
        write_book_to_dir(
            staging,
            "X",
            3,
            subdir="sub",
            cover_note="nested_7z_test.7z > sub/ (ただのサブフォルダ)\nTEST: フォルダ型バッジ",
        )

    with tempfile.TemporaryDirectory() as staging:
        build(staging)
        pack_7z(sevenz, staging, path)
    print(f"  wrote {path}")


def build_rar_in_zip(path, rar):
    """ZIP > inner.rar (+ 直下画像)。zip/rar の最小ケース。"""
    def rar_inner(staging):
        write_book_to_dir(
            staging,
            "ZR",
            3,
            cover_note="rar_in_zip.zip > inner.rar\n3 頁\nTEST: ZIP 内 rar の最小ケース",
        )

    e = [
        (
            "front.png",
            page_png(
                "S",
                1,
                1,
                cover_note="rar_in_zip.zip 直下画像\nTEST: 変換提案→展開後 inner.rar が本になる",
            ),
        ),
        ("inner.rar", make_rar_bytes(rar, rar_inner)),
    ]
    write_zip(path, e)


def build_zip_in_rar(path, rar):
    """rar > inner.zip (+ 直下画像)。rar/zip の最小ケース。"""

    def build(staging):
        for name, data in book_pages(
            "ZR",
            2,
            cover_note="zip_in_rar.rar 直下 2 頁\nTEST: rar 変換時に inner.zip も展開される",
        ):
            with open(os.path.join(staging, name), "wb") as f:
                f.write(data)
        inner = make_inner_zip(
            book_pages(
                "ZZ",
                3,
                cover_note="zip_in_rar.rar > inner.zip\n3 頁\nTEST: rar 内の zip の最小ケース",
            )
        )
        with open(os.path.join(staging, "inner.zip"), "wb") as f:
            f.write(inner)

    with tempfile.TemporaryDirectory() as staging:
        build(staging)
        pack_rar(rar, staging, path)
    print(f"  wrote {path}")


def build_nested_rar_test(path, rar):
    """rar > inner.rar。rar 入れ子の最小ケース。"""

    def inner(staging):
        write_book_to_dir(
            staging,
            "NR",
            3,
            cover_note="nested_rar_test.rar > inner.rar\n3 頁\nTEST: rar in rar も展開される",
        )

    inner_bytes = make_rar_bytes(rar, inner)

    def build(staging):
        with open(os.path.join(staging, "inner.rar"), "wb") as f:
            f.write(inner_bytes)
        for name, data in book_pages(
            "ZR",
            2,
            cover_note="nested_rar_test.rar 直下 2 頁\nTEST: 変換後 inner.rar が本として現れる",
        ):
            with open(os.path.join(staging, name), "wb") as f:
                f.write(data)

    with tempfile.TemporaryDirectory() as staging:
        build(staging)
        pack_rar(rar, staging, path)
    print(f"  wrote {path}")


def main():
    sevenz = find_7z()
    rar = find_rar()
    if not sevenz:
        print("ERROR: 7z.exe が見つかりません (7-Zip をインストールしてください)。")
        sys.exit(1)
    os.makedirs(OUT_DIR, exist_ok=True)
    print(f"Generating nested-archive test files into {OUT_DIR}")
    print(f"  7z : {sevenz}")
    print(f"  rar: {rar or '(not found)'}")

    build_foreign_in_zip(os.path.join(OUT_DIR, "foreign_in_zip.zip"), sevenz, rar)
    build_nested_7z_test(os.path.join(OUT_DIR, "nested_7z_test.7z"), sevenz)

    if rar:
        build_rar_in_zip(os.path.join(OUT_DIR, "rar_in_zip.zip"), rar)
        build_zip_in_rar(os.path.join(OUT_DIR, "zip_in_rar.rar"), rar)
        build_nested_rar_test(os.path.join(OUT_DIR, "nested_rar_test.rar"), rar)
    else:
        print(
            "\n  NOTE: WinRAR (Rar.exe) が見つからないため RAR サンプル\n"
            "        (rar_in_zip.zip / zip_in_rar.rar / nested_rar_test.rar) は\n"
            "        スキップしました。WinRAR をインストールして再実行すると生成されます:\n"
            "          winget install RARLab.WinRAR\n"
            "        (RAR の作成は WinRAR 専用機能のため 7-Zip では代替できません。\n"
            "         foreign_in_zip.zip の rar 部分も WinRAR がある環境で再生成すると追加されます)"
        )
    print("Done.")


if __name__ == "__main__":
    main()
