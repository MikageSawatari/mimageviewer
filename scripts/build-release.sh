#!/usr/bin/env bash
# mimageviewer release ビルドラッパー (Bash)
#
# トレイ常駐などで `mimageviewer.exe` が動いていると、cargo がリンク段階で
# `target/release/mimageviewer.exe` を上書きできず LNK1104 (アクセスが拒否
# されました) で失敗する。本スクリプトは:
#   1. 実行中の mimageviewer.exe / mimageviewer-susie32.exe を `taskkill` で停止
#   2. ファイルハンドル解放を待つ
#   3. cargo build --release --bin mimageviewer (引数は透過)
# を順に実行する。
#
# 使い方:
#   bash scripts/build-release.sh
#   bash scripts/build-release.sh --features foo

set -euo pipefail

targets=("mimageviewer.exe" "mimageviewer-susie32.exe")
killed=0
for name in "${targets[@]}"; do
    # tasklist は exit 0 で「該当なし」を返す系統と stderr に書く系統があるため、
    # Windows 上では taskkill /IM ... /F を直接打って exit code で分岐する。
    if taskkill //IM "$name" //F >/dev/null 2>&1; then
        echo "[build-release] stopped $name"
        killed=1
    fi
done

if [ "$killed" = "1" ]; then
    # OS のファイルハンドル解放は数百 ms 遅れることがあるので少し待つ。
    sleep 0.5
fi

echo "[build-release] cargo build --release --bin mimageviewer $*"
cargo build --release --bin mimageviewer "$@"
