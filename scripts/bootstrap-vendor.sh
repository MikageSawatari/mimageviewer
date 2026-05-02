#!/usr/bin/env bash
# vendor/ ファイル一括セットアップスクリプト
#
# 新規 clone / vendor/ 配下が消失した直後の復旧で 1 コマンドで全部揃える。
# 各サブシステムの個別 setup-*.sh を順に呼ぶラッパー。
#
# 使い方:
#   bash scripts/bootstrap-vendor.sh           # 不足分のみ取得
#   bash scripts/bootstrap-vendor.sh --force   # 既存ファイルも再取得 (デバッグ用)
#
# 前提:
#   - gh (GitHub CLI) 認証済み
#   - rustup target add i686-pc-windows-msvc 済み (susie 32bit ワーカー用)
#   - LIBCLANG_PATH 環境変数登録済み (FFmpeg の bindgen 用、CLAUDE.md 参照)
#   - %APPDATA%/mimageviewer/models/ にインストール済み環境の ONNX があること
#     (= ONNX モデルだけは自動 DL 経路がないため、コピー元として使う)
#
# **vst3-host/mimageviewer-vst3-host.exe について**:
#   このスクリプトは触らない。ビルド済み exe を別 worktree や前回のビルド結果から
#   コピーするか、CLAUDE.md「VST3 host bridge 管理」節に従って
#     bash scripts/setup-vst3-sdk.sh
#     cmake -S crates/vst3-host -B crates/vst3-host/build -G "Visual Studio 17 2022" -A x64
#     cmake --build crates/vst3-host/build --config Release
#     cp crates/vst3-host/build/Release/mimageviewer-vst3-host.exe vendor/vst3-host/
#   を手動で実行する。SDK 取得が ~490 MB なので bootstrap には混ぜない。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

FORCE=0
if [[ "${1:-}" == "--force" ]]; then
    FORCE=1
fi

run_step() {
    local label="$1"
    local check_path="$2"
    local cmd="$3"
    echo "============================================================"
    echo "[$label]"
    if [[ $FORCE -eq 0 && -e "$check_path" ]]; then
        echo "  既に存在: $check_path (スキップ)"
        return 0
    fi
    echo "  実行: $cmd"
    eval "$cmd"
}

run_step "PDFium" "vendor/pdfium/bin/pdfium.dll" \
    "bash scripts/setup-pdfium.sh"

run_step "ONNX Runtime" "vendor/ort/onnxruntime.dll" \
    "bash scripts/setup-ort.sh"

run_step "FFmpeg LGPL shared" "vendor/ffmpeg/bin/avcodec-61.dll" \
    "bash scripts/setup-ffmpeg.sh"

run_step "Susie 32bit worker" "vendor/susie-worker/mimageviewer-susie32.exe" \
    "bash scripts/setup-susie-worker.sh"

# ONNX モデル: APPDATA からコピー (= ローカル mIV インストール環境がないと取れない)
echo "============================================================"
echo "[ONNX models]"
APPDATA_MODELS="${APPDATA:-$HOME/AppData/Roaming}/mimageviewer/models"
MODELS_NEEDED=(
    anime_classifier_mobilenetv3.onnx
    realesrgan_x4plus.onnx
    realesrgan_x4plus_anime_6b.onnx
    realesr_general_x4v3.onnx
    realcugan_4x_conservative.onnx
    4x_NMKD-Siax_200k.onnx
    dejpg_realplksr_otf.onnx
    migan.onnx
)
mkdir -p vendor/models
missing_models=()
for m in "${MODELS_NEEDED[@]}"; do
    if [[ $FORCE -eq 0 && -f "vendor/models/$m" ]]; then
        continue
    fi
    src="$APPDATA_MODELS/$m"
    if [[ -f "$src" ]]; then
        cp "$src" "vendor/models/"
        echo "  copied: $m"
    else
        missing_models+=("$m")
    fi
done
if [[ ${#missing_models[@]} -gt 0 ]]; then
    echo "  ⚠️  以下のモデルが APPDATA に無いため手動配置が必要:"
    for m in "${missing_models[@]}"; do
        echo "    - vendor/models/$m"
    done
    echo "    (= mIV を一度インストール / 起動して APPDATA に展開させた後に再実行する)"
fi

# vst3-host exe: bootstrap の対象外 (上のヘッダコメント参照)
echo "============================================================"
echo "[VST3 host bridge]"
if [[ -f "vendor/vst3-host/mimageviewer-vst3-host.exe" ]]; then
    echo "  既に存在: vendor/vst3-host/mimageviewer-vst3-host.exe"
else
    echo "  ⚠️  vendor/vst3-host/mimageviewer-vst3-host.exe が未配置。"
    echo "      他 worktree やバックアップに既存ビルド済み exe があればコピー、"
    echo "      無ければ CMake で再ビルド (CLAUDE.md「VST3 host bridge 管理」節参照)。"
fi

echo "============================================================"
echo "完了。残っている警告があれば対応した上で:"
echo "  bash scripts/build-release.sh   (または PowerShell scripts/build-release.ps1)"
