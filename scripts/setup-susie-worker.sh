#!/usr/bin/env bash
# Susie 32bit ワーカー exe のセットアップスクリプト
#
# 使い方:
#   bash scripts/setup-susie-worker.sh
#
# 前提:
#   - `rustup target add i686-pc-windows-msvc` 済みであること
#   - 32bit MSVC ビルドツールが使えること
#
# 出力先: vendor/susie-worker/mimageviewer-susie32.exe
#   この exe は include_bytes! でメインの mimageviewer.exe に埋め込まれ、
#   初回起動時に %APPDATA%\mimageviewer\mimageviewer-susie32.exe へ展開される。
#
# メイン exe のリリースビルド前には必ず実行しておくこと。

set -euo pipefail

cd "$(dirname "$0")/.."

echo "=== Building 32bit Susie worker ==="
cargo build --release --target i686-pc-windows-msvc -p mimageviewer-susie32

SRC="target/i686-pc-windows-msvc/release/mimageviewer-susie32.exe"
DST_DIR="vendor/susie-worker"
DST="$DST_DIR/mimageviewer-susie32.exe"

if [ ! -f "$SRC" ]; then
    echo "ERROR: Build output not found: $SRC" >&2
    exit 1
fi

mkdir -p "$DST_DIR"
cp "$SRC" "$DST"

SIZE=$(stat -c%s "$DST" 2>/dev/null || stat -f%z "$DST")
echo ""
echo "=== Setup complete ==="
echo "Worker exe: $(pwd)/$DST"
echo "Size: $SIZE bytes"
echo ""
echo "Run 'cargo build --release' to embed the worker into the main exe."
