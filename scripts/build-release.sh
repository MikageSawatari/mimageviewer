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

# 2 段階ビルド (ランチャー方式):
#   1. core (本体、FFmpeg DLL に静的依存) を `mimageviewer-core.exe` として生成
#   2. launcher (FFmpeg 非依存、core + 5 DLL を include_bytes! で内包) を
#      `mimageviewer.exe` として生成。配布する単体 exe はこちら。
#
# cargo は同一ワークスペース内 bin の依存順序を表現できないため、明示的に 2 回呼ぶ。
echo "[build-release] (1/2) cargo build --release --bin mimageviewer-core $*"
cargo build --release --bin mimageviewer-core "$@"

echo "[build-release] (2/2) cargo build --release -p mimageviewer-launcher --bin mimageviewer $*"
cargo build --release -p mimageviewer-launcher --bin mimageviewer "$@"
