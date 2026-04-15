#!/usr/bin/env bash
# AI モデルファイルを GitHub Releases からダウンロードして vendor/models/ に配置する。
# ビルド前に1度だけ実行すればよい。
#
# Usage:
#   bash scripts/setup-models.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DEST="$ROOT_DIR/vendor/models"

BASE_URL="https://github.com/MikageSawatari/mimageviewer/releases/download/models-v1"

MODELS=(
    "anime_classifier_mobilenetv3.onnx"
    "realesrgan_x4plus.onnx"
    "realesrgan_x4plus_anime_6b.onnx"
    "realesr_general_x4v3.onnx"
    "realcugan_4x_conservative.onnx"
    "dejpg_realplksr_otf.onnx"
    "migan.onnx"
)

mkdir -p "$DEST"

for model in "${MODELS[@]}"; do
    dest_file="$DEST/$model"
    if [ -f "$dest_file" ] && [ -s "$dest_file" ]; then
        echo "Already exists: $model"
        continue
    fi
    echo "Downloading: $model"
    curl -L -o "$dest_file" "$BASE_URL/$model"
    echo "  -> $(wc -c < "$dest_file") bytes"
done

echo ""
echo "All models downloaded to $DEST"
