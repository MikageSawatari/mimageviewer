#!/usr/bin/env bash
# PDFium DLL のダウンロード・セットアップスクリプト
#
# 使い方:
#   bash scripts/setup-pdfium.sh          # 最新版をダウンロード
#   bash scripts/setup-pdfium.sh check    # 新しいバージョンがあるか確認のみ
#
# 前提:
#   - gh (GitHub CLI) がインストール済みであること
#   - tar コマンドが使えること (Git Bash / MSYS2 等)
#
# 出力先: vendor/pdfium/bin/pdfium.dll
#   この DLL は include_bytes! で exe に埋め込まれる

set -euo pipefail

REPO="bblanchon/pdfium-binaries"
ASSET="pdfium-win-x64.tgz"
VENDOR_DIR="vendor/pdfium"
VERSION_FILE="$VENDOR_DIR/VERSION"

cd "$(dirname "$0")/.."

# ── 最新リリースのタグを取得 ──
latest_tag=$(gh release list --repo "$REPO" --limit 1 --json tagName --jq '.[0].tagName')
echo "Latest release: $latest_tag"

# ── 現在のバージョン確認 ──
if [ -f "$VERSION_FILE" ]; then
    current_build=$(grep '^BUILD=' "$VERSION_FILE" | cut -d= -f2)
    echo "Current version: BUILD=$current_build"
else
    current_build=""
    echo "Current version: (not installed)"
fi

# ── check モード: 確認のみ ──
if [ "${1:-}" = "check" ]; then
    if [ -n "$current_build" ] && echo "$latest_tag" | grep -q "$current_build"; then
        echo "Up to date."
    else
        echo "New version available: $latest_tag (current: BUILD=${current_build:-none})"
    fi
    exit 0
fi

# ── ダウンロード & 展開 ──
echo "Downloading $ASSET from $latest_tag ..."
mkdir -p "$VENDOR_DIR"
cd "$VENDOR_DIR"

gh release download "$latest_tag" --repo "$REPO" --pattern "$ASSET" --clobber
tar xzf "$ASSET"
rm -f "$ASSET"

echo ""
echo "=== Setup complete ==="
echo "DLL: $(pwd)/bin/pdfium.dll"
echo "Version: $(cat VERSION)"
echo ""
echo "Run 'cargo build --release' to embed the DLL into the exe."
