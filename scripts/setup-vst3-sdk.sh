#!/usr/bin/env bash
# Steinberg VST3 SDK セットアップスクリプト
#
# 用途:
#   crates/vst3-host (C++ bridge プロセス) のビルドに必要な VST3 SDK を
#   vendor/vst3sdk/ に取得する。
#
#   VST3 SDK は 2025-10-20 (v3.8.0) から MIT ライセンス化されているため、
#   PDFium / FFmpeg と同様に開発者各自が取得して vendor/ に置く運用とする
#   (.gitignore 済み)。
#
# 使い方:
#   bash scripts/setup-vst3-sdk.sh           # 既定バージョン (master) を clone
#   bash scripts/setup-vst3-sdk.sh check     # ローカルが最新と一致するか確認のみ
#
# 前提:
#   - git が PATH にあること
#   - VST3 SDK 3.8.0 (タグ "3.8.0" 以降) を要求 (MIT ライセンス)
#
# 出力先:
#   vendor/vst3sdk/                          # SDK 全体 (再帰的サブモジュール込み)
#   vendor/vst3sdk/LICENSE.txt               # MIT ライセンス本文
#
set -euo pipefail

REPO_URL="https://github.com/steinbergmedia/vst3sdk.git"
TARGET_DIR="vendor/vst3sdk"
# 最低要求バージョン (これ以降が MIT)。実際は master を取って最新を使う。
MIN_VERSION="3.8.0"

cd "$(dirname "$0")/.."

mode="${1:-fetch}"

case "$mode" in
    check)
        if [[ ! -d "$TARGET_DIR" ]]; then
            echo "VST3 SDK が未配置です。bash scripts/setup-vst3-sdk.sh を実行してください。"
            exit 1
        fi
        if [[ ! -f "$TARGET_DIR/LICENSE.txt" ]]; then
            echo "WARN: $TARGET_DIR/LICENSE.txt が見当たりません。SDK が壊れている可能性があります。"
            exit 1
        fi
        if ! grep -q "MIT License" "$TARGET_DIR/LICENSE.txt"; then
            echo "ERROR: $TARGET_DIR/LICENSE.txt が MIT License ではありません。"
            echo "       VST3 SDK 3.8.0 (2025-10-20) 以降を取得し直してください。"
            exit 1
        fi
        cd "$TARGET_DIR"
        local_head=$(git rev-parse HEAD)
        git fetch --quiet origin
        remote_head=$(git rev-parse origin/master)
        if [[ "$local_head" == "$remote_head" ]]; then
            echo "OK: VST3 SDK は最新 ($local_head) です。"
        else
            echo "更新あり: ローカル $local_head -> リモート $remote_head"
            echo "更新するには bash scripts/setup-vst3-sdk.sh を再実行してください。"
        fi
        ;;
    fetch|"")
        if [[ -d "$TARGET_DIR" ]]; then
            echo "$TARGET_DIR が既に存在します。最新を pull します。"
            (
                cd "$TARGET_DIR"
                git fetch --quiet origin
                git reset --hard origin/master
                git submodule update --init --recursive
            )
        else
            echo "$REPO_URL を $TARGET_DIR に clone します..."
            git clone --recursive "$REPO_URL" "$TARGET_DIR"
        fi
        echo ""
        echo "ライセンス確認:"
        head -5 "$TARGET_DIR/LICENSE.txt"
        echo ""
        echo "完了。VST3 SDK $(cd "$TARGET_DIR" && git describe --always) を取得しました。"
        echo "最低要求バージョン: $MIN_VERSION (MIT License)"
        ;;
    *)
        echo "Usage: $0 [fetch|check]"
        exit 1
        ;;
esac
