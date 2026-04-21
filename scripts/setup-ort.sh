#!/usr/bin/env bash
# ONNX Runtime (DirectML) DLL のダウンロード・セットアップスクリプト
#
# 使い方:
#   bash scripts/setup-ort.sh                # デフォルトバージョンをダウンロード
#   bash scripts/setup-ort.sh 1.24.2         # バージョン指定
#
# 前提:
#   - curl と unzip コマンドが使えること (Git Bash / MSYS2 等)
#
# 出力先:
#   vendor/ort/onnxruntime.dll
#   vendor/ort/onnxruntime_providers_shared.dll
#   vendor/ort/VERSION
#   これらは include_bytes! で exe に埋め込まれ、初回起動時に
#   %APPDATA%/mimageviewer/ へ展開される (PDFium と同じ流儀)。
#
# バージョン互換性:
#   ort クレート v2.0.0-rc.12 は ONNX Runtime 1.24.2 (ms@1.24.2) の
#   C API と ABI 互換。pyke のビルドスクリプトの dist.txt を参照。
#   ort クレートをアップデートしたら対応する ORT バージョンを確認すること。

set -euo pipefail

VERSION="${1:-1.24.2}"
VENDOR_DIR="vendor/ort"
NUPKG_URL="https://globalcdn.nuget.org/packages/microsoft.ml.onnxruntime.directml.${VERSION}.nupkg"

cd "$(dirname "$0")/.."

mkdir -p "$VENDOR_DIR"
cd "$VENDOR_DIR"

echo "Downloading Microsoft.ML.OnnxRuntime.DirectML $VERSION ..."
curl -fsSL -o ort.nupkg "$NUPKG_URL"

echo "Extracting win-x64 DLLs ..."
unzip -o -j ort.nupkg "runtimes/win-x64/native/onnxruntime.dll" -d .
unzip -o -j ort.nupkg "runtimes/win-x64/native/onnxruntime_providers_shared.dll" -d .

rm -f ort.nupkg
echo "$VERSION" > VERSION

echo ""
echo "=== Setup complete ==="
echo "DLL: $(pwd)/onnxruntime.dll"
echo "DLL: $(pwd)/onnxruntime_providers_shared.dll"
echo "Version: $(cat VERSION)"
echo ""
echo "Run 'cargo build --release' to embed the DLLs into the exe."
