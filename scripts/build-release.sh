#!/usr/bin/env bash
# mimageviewer release ビルドラッパー (Bash)
#
# トレイ常駐などで `mimageviewer.exe` が動いていると、cargo がリンク段階で
# `target/release/mimageviewer.exe` を上書きできず LNK1104 (アクセスが拒否
# されました) で失敗する。本スクリプトは:
#   1. 実行中の mimageviewer.exe / mimageviewer-susie32.exe を `taskkill` で停止
#   2. ファイルハンドル解放を待つ
#   3. VST3 C++ bridge を再ビルド (core が include_bytes! で内包するため)
#   4. core → launcher の 2 段階 cargo build (CARGO_INCREMENTAL=0)
#   5. APPDATA 上の stale VST3 bridge cache を削除 (次回起動時に再展開させる)
# を順に実行する。
#
# 使い方:
#   bash scripts/build-release.sh
#   bash scripts/build-release.sh --features foo
#
# **PowerShell ラッパーとの整合**: `scripts/build-release.ps1` と同じステップを
# 行うように T56 (2026-05-16) で揃えた。VST3 bridge 再ビルド + cache 削除が
# 抜けると stale bridge を embed してしまい、配布版で「VST3 が動かない」回帰
# になる。Windows 上で `cmake` が無い場合はラッパーが警告を出して bridge 再
# ビルドをスキップする (= 旧挙動と同じ。CI / 開発機で明示)。

set -euo pipefail

targets=("mimageviewer.exe" "mimageviewer-susie32.exe" "mimageviewer-core.exe" "mimageviewer-vst3-host.exe")
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

# T56 (Codex R-REL-002): VST3 C++ bridge を再ビルドしてから cargo を呼ぶ。
# core (mimageviewer-core.exe) が `include_bytes!("../../vendor/vst3-host/
# mimageviewer-vst3-host.exe")` で内包するため、bridge を更新せずに cargo を
# 走らせると stale bridge を embed して配布版が「VST3 起動しない」になる。
SKIP_VST3="${SKIP_VST3_BRIDGE:-0}"
if [ "$SKIP_VST3" != "1" ]; then
    if command -v cmake >/dev/null 2>&1; then
        VST3_BUILD_DIR="crates/vst3-host/build"
        VST3_VENDOR_EXE="vendor/vst3-host/mimageviewer-vst3-host.exe"
        VST3_SDK_LICENSE="vendor/vst3sdk/LICENSE.txt"
        if [ ! -f "$VST3_SDK_LICENSE" ]; then
            if [ -f "$VST3_VENDOR_EXE" ]; then
                echo "[build-release] WARN: vendor/vst3sdk が無い。既存の $VST3_VENDOR_EXE を再利用 (= bridge 変更は反映されない)"
            else
                echo "[build-release] ERROR: vendor/vst3sdk と $VST3_VENDOR_EXE の両方が無い。bash scripts/setup-vst3-sdk.sh を実行するか SKIP_VST3_BRIDGE=1 で skip してください" >&2
                exit 1
            fi
        else
            if [ ! -f "$VST3_BUILD_DIR/CMakeCache.txt" ]; then
                echo "[build-release] configuring VST3 bridge (cmake)"
                cmake -S crates/vst3-host -B "$VST3_BUILD_DIR" -G "Visual Studio 18 2026" -A x64
            fi
            echo "[build-release] (1/3) cmake --build $VST3_BUILD_DIR --config Release"
            cmake --build "$VST3_BUILD_DIR" --config Release
            if [ ! -f "$VST3_VENDOR_EXE" ]; then
                echo "[build-release] ERROR: cmake build did not produce $VST3_VENDOR_EXE" >&2
                exit 1
            fi
        fi
    else
        if [ -f "vendor/vst3-host/mimageviewer-vst3-host.exe" ]; then
            echo "[build-release] WARN: cmake が PATH に無い。既存 vendor/vst3-host/mimageviewer-vst3-host.exe を再利用 (= bridge 変更は反映されない)"
        else
            echo "[build-release] ERROR: cmake が無く、vendor/vst3-host/mimageviewer-vst3-host.exe も無い。CMake をインストールするか SKIP_VST3_BRIDGE=1 を指定してください" >&2
            exit 1
        fi
    fi
else
    echo "[build-release] SKIP_VST3_BRIDGE=1 — skipping VST3 bridge rebuild"
fi

# 2 段階ビルド (ランチャー方式):
#   1. core (本体、FFmpeg DLL に静的依存) を `mimageviewer-core.exe` として生成
#   2. launcher (FFmpeg 非依存、core + FFmpeg DLL を include_bytes! で内包) を
#      `mimageviewer.exe` として生成。配布する単体 exe はこちら。
#
# cargo は同一ワークスペース内 bin の依存順序を表現できないため、明示的に 2 回呼ぶ。
echo "[build-release] (2/3) CARGO_INCREMENTAL=0 cargo build --release --bin mimageviewer-core $*"
CARGO_INCREMENTAL=0 cargo build --release --bin mimageviewer-core "$@"

echo "[build-release] (3/3) CARGO_INCREMENTAL=0 cargo build --release -p mimageviewer-launcher --bin mimageviewer $*"
CARGO_INCREMENTAL=0 cargo build --release -p mimageviewer-launcher --bin mimageviewer "$@"

# T56: APPDATA 上の展開済み VST3 bridge cache を削除して、次回起動時に新 bridge を
# 展開させる。これをしないと開発機が旧 bridge を握り続け、再ビルドした bridge が
# 反映されない実害があった (PowerShell 版と同等の処理)。
#
# Codex post-merge P3 (2026-05-16): 旧コードは `set -u` 下で `$APPDATA/...` を先に展開
# していたため、`APPDATA` 未設定の CI / 非標準 shell でこの行で即座に unbound variable
# エラーになって落ちていた (= release build 自体は成功しているのに最後だけ exit 1)。
# `${APPDATA:-}` guard を最初に評価し、未設定なら掃除をスキップして正常 exit する。
if [ -n "${APPDATA:-}" ]; then
    APPDATA_VST3_BRIDGE="$APPDATA/mimageviewer/vst3/mimageviewer-vst3-host.exe"
    APPDATA_VST3_BRIDGE_HASH="$APPDATA/mimageviewer/vst3/mimageviewer-vst3-host.exe.sha256"
    for cache in "$APPDATA_VST3_BRIDGE" "$APPDATA_VST3_BRIDGE_HASH"; do
        if [ -f "$cache" ]; then
            if rm -f "$cache"; then
                echo "[build-release] removed stale extracted VST3 bridge cache: $cache"
            fi
        fi
    done
fi
