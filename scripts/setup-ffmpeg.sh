#!/usr/bin/env bash
# FFmpeg LGPL shared build のダウンロード・セットアップスクリプト。
#
# 使い方:
#   bash scripts/setup-ffmpeg.sh           # 最新の LGPL shared ビルドをダウンロード
#   bash scripts/setup-ffmpeg.sh check     # 新しいバージョンがあるか確認のみ
#
# 前提:
#   - gh (GitHub CLI), unzip, curl が使えること (Git Bash / MSYS2 等)
#
# 出力:
#   vendor/ffmpeg/bin/{avcodec,avformat,avutil,swscale,swresample}-*.dll
#   vendor/ffmpeg/include/  (FFmpeg ヘッダ)
#   vendor/ffmpeg/lib/      (import library .lib)
#   vendor/ffmpeg/LICENSE.txt
#   vendor/ffmpeg/VERSION
#
# DLL は include_bytes! で exe に埋め込まれ、初回起動時に
# %APPDATA%/mimageviewer/ffmpeg/ へ展開される (PDFium / ONNX Runtime と同じ流儀)。
# ヘッダ・lib は ffmpeg-the-third のビルド時に FFMPEG_DIR 経由で参照される。
#
# ライセンス:
#   BtbN の LGPL shared build を使用。LGPLv2.1 表記とソース提供義務を満たすため、
#   配布物の「ソフトウェア情報」と installer/readme.txt に LGPL 通知を入れること。
#   詳細は CLAUDE.md の「FFmpeg ライセンス対応」節を参照。

set -euo pipefail

REPO="BtbN/FFmpeg-Builds"
# n7.1 系の最新 LGPL shared build を狙う。新版を使いたければ ASSET_GLOB を変える。
ASSET_GLOB="ffmpeg-n7.1*-win64-lgpl-shared-7.1.zip"
VENDOR_DIR="vendor/ffmpeg"
VERSION_FILE="$VENDOR_DIR/VERSION"

cd "$(dirname "$0")/.."

# ── 最新リリースのアセット一覧から ASSET_GLOB に合致するものを探す ──
echo "Querying $REPO for asset matching $ASSET_GLOB ..."
asset_name=$(gh release list --repo "$REPO" --limit 5 --json tagName \
    | grep -Eo '"tagName":"[^"]+"' \
    | head -1 \
    | sed 's/"tagName":"//; s/"$//')
if [ -z "$asset_name" ]; then
    echo "Failed to query latest tag." >&2
    exit 1
fi
latest_tag="$asset_name"
echo "Latest tag: $latest_tag"

# 該当アセット名を解決 (latest tag 内の glob にマッチするもの)
matched=$(gh release view "$latest_tag" --repo "$REPO" --json assets \
    --jq ".assets[].name" 2>/dev/null \
    | grep -E "^${ASSET_GLOB//\*/.*}$" \
    | head -1 || true)
if [ -z "$matched" ]; then
    echo "No asset matching $ASSET_GLOB in $latest_tag." >&2
    echo "Try a different ASSET_GLOB (e.g. ffmpeg-n7.0*-win64-lgpl-shared*.zip)." >&2
    exit 1
fi
echo "Matched asset: $matched"

# ── 現在のバージョン確認 ──
if [ -f "$VERSION_FILE" ]; then
    current_asset=$(cat "$VERSION_FILE")
    echo "Current: $current_asset"
else
    current_asset=""
    echo "Current: (not installed)"
fi

# ── check モード ──
if [ "${1:-}" = "check" ]; then
    if [ "$current_asset" = "$matched" ]; then
        echo "Up to date."
    else
        echo "New version available: $matched (current: ${current_asset:-none})"
    fi
    exit 0
fi

# ── ダウンロード ──
mkdir -p "$VENDOR_DIR"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "Downloading $matched ..."
gh release download "$latest_tag" --repo "$REPO" --pattern "$matched" \
    --dir "$TMP" --clobber

echo "Extracting ..."
unzip -q "$TMP/$matched" -d "$TMP"

# 展開後ディレクトリ名は "ffmpeg-n7.1.X-XXX-win64-lgpl-shared-7.1" 等
src_root=$(find "$TMP" -mindepth 1 -maxdepth 1 -type d | head -1)
if [ -z "$src_root" ] || [ ! -d "$src_root/bin" ]; then
    echo "Unexpected zip layout: no bin/ under $src_root" >&2
    exit 1
fi

# ── 必要 DLL のみをコピー (avdevice/avfilter/postproc/ffmpeg.exe 等は不要) ──
mkdir -p "$VENDOR_DIR/bin" "$VENDOR_DIR/include" "$VENDOR_DIR/lib"

echo "Copying DLLs ..."
for prefix in avcodec avformat avutil swscale swresample; do
    found=$(find "$src_root/bin" -maxdepth 1 -name "${prefix}-*.dll" -type f | head -1)
    if [ -z "$found" ]; then
        echo "Missing required DLL: ${prefix}-*.dll" >&2
        exit 1
    fi
    cp -f "$found" "$VENDOR_DIR/bin/$(basename "$found")"
done

echo "Copying import libraries ..."
for prefix in avcodec avformat avutil swscale swresample; do
    found=$(find "$src_root/lib" -maxdepth 1 -name "${prefix}.lib" -type f | head -1)
    if [ -z "$found" ]; then
        echo "Missing required lib: ${prefix}.lib" >&2
        exit 1
    fi
    cp -f "$found" "$VENDOR_DIR/lib/$(basename "$found")"
done

echo "Copying headers ..."
# include 配下を丸ごとコピー (libavcodec/, libavformat/, libavutil/, libswscale/, libswresample/)
cp -rf "$src_root/include/." "$VENDOR_DIR/include/"

echo "Copying LICENSE ..."
if [ -f "$src_root/LICENSE.txt" ]; then
    cp -f "$src_root/LICENSE.txt" "$VENDOR_DIR/LICENSE.txt"
elif [ -f "$src_root/LICENSE" ]; then
    cp -f "$src_root/LICENSE" "$VENDOR_DIR/LICENSE.txt"
fi

echo "$matched" > "$VERSION_FILE"

# ── 結果サマリ ──
echo ""
echo "=== Setup complete ==="
echo "Version: $(cat "$VERSION_FILE")"
echo "DLLs:"
ls "$VENDOR_DIR/bin"
echo "Libs:"
ls "$VENDOR_DIR/lib"
echo ""
echo "次は cargo build --release で DLL を exe に埋め込みます。"
echo "ライセンス: LGPLv2.1。配布時は「ソフトウェア情報」に表記、"
echo "mikage.to にソース tarball を配置してください。"
