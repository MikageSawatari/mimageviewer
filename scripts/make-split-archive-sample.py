"""Pack the split/trim sample pages into every archive shape mIV supports.

Companion to make-split-trim-sample.py. Covers the container kinds that reach
the landscape-split code by different routes:

  split-plain.zip       pages at the zip root
  split-one-folder.zip  pages under one folder -- the collapse_redundant case,
                        where the container the app opens is not the one asked for
  split-nested.zip      an outer zip holding two inner zips
  split-converted.7z    goes through the RAR/7z -> ZIP conversion first
  split-converted.rar   same, other format

Run:  python make-split-archive-sample.py <pages-dir> <output-dir>
"""
import os
import shutil
import subprocess
import sys
import zipfile

SEVEN_ZIP = r"C:\Program Files\7-Zip\7z.exe"
RAR = r"C:\Program Files\WinRAR\Rar.exe"


def zip_pages(path, pages, prefix="", compression=zipfile.ZIP_DEFLATED):
    with zipfile.ZipFile(path, "w", compression) as archive:
        for source, name in pages:
            archive.write(source, prefix + name)


def main():
    src, out = sys.argv[1], sys.argv[2]
    os.makedirs(out, exist_ok=True)
    names = sorted(name for name in os.listdir(src) if name.endswith(".png"))
    pages = [(os.path.join(src, name), name) for name in names]
    if not pages:
        raise SystemExit(f"no pages in {src}")

    zip_pages(os.path.join(out, "split-plain.zip"), pages)
    zip_pages(os.path.join(out, "split-one-folder.zip"), pages, prefix="book/")

    inner = []
    for index, chunk in enumerate((pages[:3], pages[3:]), start=1):
        if not chunk:
            continue
        path = os.path.join(out, f"_inner_{index}.zip")
        zip_pages(path, chunk)
        inner.append((path, f"vol{index:02d}.zip"))
    # The outer archive is stored, so the inner zips stay readable as entries.
    zip_pages(os.path.join(out, "split-nested.zip"), inner, compression=zipfile.ZIP_STORED)
    for path, _ in inner:
        os.remove(path)

    for tool, args, name in (
        (SEVEN_ZIP, ["a", "-t7z"], "split-converted.7z"),
        (RAR, ["a", "-ep"], "split-converted.rar"),
    ):
        if not os.path.exists(tool):
            print("skip (not installed):", name)
            continue
        target = os.path.join(out, name)
        if os.path.exists(target):
            os.remove(target)
        subprocess.run(
            [tool, *args, target, os.path.join(src, "*.png")],
            check=True,
            stdout=subprocess.DEVNULL,
        )

    for name in sorted(os.listdir(out)):
        print(f"  {name}  {os.path.getsize(os.path.join(out, name))} bytes")


main()
