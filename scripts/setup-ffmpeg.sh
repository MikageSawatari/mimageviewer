#!/usr/bin/env bash
# FFmpeg LGPL shared build のダウンロード・セットアップスクリプト。
#
# 使い方:
#   bash scripts/setup-ffmpeg.sh           # 最新の版付き LGPL shared ビルドをダウンロード
#   bash scripts/setup-ffmpeg.sh check     # 新しい版付きビルドがあるか確認のみ
#
# 前提:
#   - gh (GitHub CLI), unzip, curl が使えること (Git Bash / MSYS2 等)
#
# 出力:
#   vendor/ffmpeg/bin/{avcodec,avformat,avutil,avfilter,swscale,swresample}-*.dll
#   vendor/ffmpeg/include/  (FFmpeg ヘッダ)
#   vendor/ffmpeg/lib/      (import library .lib)
#   vendor/ffmpeg/LICENSE.txt
#   vendor/ffmpeg/VERSION
#
# 6 DLL は launcher (`crates/launcher/`) が include_bytes! で内包し、起動時に
# %APPDATA%/mimageviewer/runtime/<version>/ へ展開してから本体 (mimageviewer-core.exe)
# を spawn する。本体は通常リンクなので Windows ローダが exe 同居 DLL を解決する。
# ヘッダ・lib は ffmpeg-the-third のビルド時に FFMPEG_DIR 経由で参照される。
#
# ライセンス:
#   BtbN の LGPL shared build を使用。LGPLv3-or-later 表記とソース提供義務を満たすため、
#   配布物の「ソフトウェア情報」と installer/readme.txt に LGPL 通知を入れること。
#   詳細は CLAUDE.md の「FFmpeg ライセンス対応」節を参照。

set -euo pipefail

REPO="BtbN/FFmpeg-Builds"
# n7.1 系の最新 LGPL shared build を狙う。新版を使いたければ ASSET_GLOB を変える。
ASSET_GLOB="ffmpeg-n7.1*-win64-lgpl-shared-7.1.zip"
RELEASE_TAG_GLOB="autobuild-*"
VENDOR_DIR="vendor/ffmpeg"
VERSION_FILE="$VENDOR_DIR/VERSION"

cd "$(dirname "$0")/.."

# ── 最新 autobuild の版付きアセットから ASSET_GLOB に合致するものを探す ──
# BtbN の tag `latest` はファイル名に `-latest-` を含むローリング資産で、同じ URL の
# 中身が更新される。VERSION と LGPL 対応ソースを一意にするため、日付付きの
# `autobuild-*` release にあるコミット hash 込みの版付き資産だけを採用する。
echo "Querying $REPO for versioned asset matching $ASSET_GLOB ..."
release_tags=$(gh release list --repo "$REPO" --limit 20 --json tagName --jq '.[].tagName')

# 新しい tag から順に見て、**該当アセットを実際に持っている**最初の tag を採用する。
# 最新 tag だけを見ると、BtbN が次のメジャーへ移って n7.1 資産を出さなくなった日に、
# 固定中のバージョンがまだ古い tag に存在していても取得できなくなる (2026-08-18 に CI が
# これで停止した)。tag の新しさではなく「欲しい資産があるか」で選ぶ。
latest_tag=""
matched=""
searched=0
while IFS= read -r candidate_tag; do
    case "$candidate_tag" in
        $RELEASE_TAG_GLOB) ;;
        *) continue ;;
    esac
    searched=$((searched + 1))
    # 該当アセット名を shell glob で解決し、rolling `-latest-` は明示的に拒否する。
    assets=$(gh release view "$candidate_tag" --repo "$REPO" --json assets --jq '.assets[].name')
    while IFS= read -r candidate; do
        case "$candidate" in
            $ASSET_GLOB)
                if [[ "$candidate" == *-latest-* ]]; then
                    continue
                fi
                matched="$candidate"
                break
                ;;
        esac
    done <<< "$assets"
    if [ -n "$matched" ]; then
        latest_tag="$candidate_tag"
        break
    fi
done <<< "$release_tags"

if [ -z "$latest_tag" ]; then
    echo "No versioned asset matching $ASSET_GLOB in the newest $searched $RELEASE_TAG_GLOB releases." >&2
    echo "Try a different ASSET_GLOB (e.g. ffmpeg-n8.0*-win64-lgpl-shared*.zip)." >&2
    exit 1
fi
echo "Newest versioned tag carrying the asset: $latest_tag"
if [[ "$matched" == *-latest-* ]]; then
    echo "Refusing rolling BtbN asset: $matched" >&2
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

# ── 必要 DLL のみをコピー (avdevice/postproc/ffmpeg.exe 等は不要) ──
mkdir -p "$VENDOR_DIR/bin" "$VENDOR_DIR/include" "$VENDOR_DIR/lib"

echo "Copying DLLs ..."
for prefix in avcodec avformat avutil avfilter swscale swresample; do
    found=$(find "$src_root/bin" -maxdepth 1 -name "${prefix}-*.dll" -type f | head -1)
    if [ -z "$found" ]; then
        echo "Missing required DLL: ${prefix}-*.dll" >&2
        exit 1
    fi
    cp -f "$found" "$VENDOR_DIR/bin/$(basename "$found")"
done

echo "Copying import libraries ..."
for prefix in avcodec avformat avutil avfilter swscale swresample; do
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
echo "ライセンス: LGPLv3-or-later。配布時は「ソフトウェア情報」に表記、"
echo "docs/ffmpeg-lgpl-source-distribution.md に従って対応ソース情報を確認してください。"
