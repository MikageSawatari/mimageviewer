#!/usr/bin/env bash
# TensorRT バックエンド用ランタイム DLL のダウンロード・セットアップスクリプト (PoC 版)
#
# 使い方:
#   bash scripts/setup-tensorrt-pack.sh
#
# 動作:
#   1. NuGet と NVIDIA 公式 redist URL から下記 4 種類の zip/nupkg をダウンロード
#      (vendor/tensorrt-cache/ にキャッシュ、再実行時は size 一致でスキップ)
#      - Microsoft.ML.OnnxRuntime.Gpu (~150 MB)
#      - CUDA cudart + cublas (~400 MB)
#      - cuDNN (~646 MB)
#      - TensorRT (~2.24 GB)
#   2. 各 zip から必要な DLL だけ抽出して %APPDATA%/mimageviewer/tensorrt/ に配置
#   3. INSTALL_OK sentinel ファイルを最後に書き込む (atomic install マーク)
#
# 前提:
#   - curl と unzip コマンドが使えること (Git Bash / MSYS2)
#   - 約 3 GB のダウンロード + 約 1.5 GB の展開後容量が必要
#   - ネットワーク帯域: 100 Mbps で約 5 分、20 Mbps で約 25 分
#
# 注: これは PoC ベンチマーク用のセットアップスクリプト。Phase 2 で実装する
#     アプリ内 DL フローは別途。

set -euo pipefail

# ───────────────────────────────────────────────
# バージョン pin (ort 2.0.0-rc.12 ↔ ONNX Runtime 1.24.2 ↔ CUDA 12.x ↔ cuDNN 9.x ↔ TRT 10.x)
# ───────────────────────────────────────────────
ORT_GPU_VERSION="1.24.2"
CUDA_CUDART_VERSION="12.9.79"
CUDA_CUBLAS_VERSION="12.9.1.4"
CUDNN_VERSION="9.21.1.3"
CUDNN_CUDA_TAG="cuda12"
TRT_VERSION="10.16.1.11"
TRT_CUDA_TAG="cuda-12.9"

# ───────────────────────────────────────────────
# ディレクトリ定義
# ───────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CACHE_DIR="$REPO_ROOT/vendor/tensorrt-cache"
EXTRACT_DIR="$REPO_ROOT/vendor/tensorrt-cache/extract"

# Windows %APPDATA% を bash 上で解決する
if [[ -n "${APPDATA:-}" ]]; then
    APPDATA_BASH="$(cygpath -u "$APPDATA" 2>/dev/null || echo "$APPDATA")"
else
    APPDATA_BASH="$HOME/AppData/Roaming"
fi
TARGET_DIR="$APPDATA_BASH/mimageviewer/tensorrt"

mkdir -p "$CACHE_DIR" "$EXTRACT_DIR" "$TARGET_DIR"

# ───────────────────────────────────────────────
# ヘルパー: download_if_missing <url> <dst_path> <expected_min_bytes>
#   既に同サイズ (>=expected_min) のファイルがあれば再 DL せずスキップ
# ───────────────────────────────────────────────
download_if_missing() {
    local url="$1"
    local dst="$2"
    local min_bytes="$3"

    if [[ -f "$dst" ]]; then
        local size
        size=$(stat -c%s "$dst" 2>/dev/null || stat -f%z "$dst" 2>/dev/null || echo 0)
        if (( size >= min_bytes )); then
            echo "  cached: $(basename "$dst") (${size} bytes)"
            return 0
        fi
        echo "  partial cache, redownloading: $(basename "$dst")"
        rm -f "$dst"
    fi

    echo "  downloading: $url"
    curl -fL --progress-bar -o "$dst" "$url"
}

# ───────────────────────────────────────────────
# 1. ダウンロード
# ───────────────────────────────────────────────
echo "═══════════════════════════════════════════════"
echo " Step 1/3: Downloads (cached in $CACHE_DIR)"
echo "═══════════════════════════════════════════════"

ORT_GPU_NUPKG="$CACHE_DIR/onnxruntime-gpu-${ORT_GPU_VERSION}.nupkg"
ORT_GPU_URL="https://globalcdn.nuget.org/packages/microsoft.ml.onnxruntime.gpu.${ORT_GPU_VERSION}.nupkg"
download_if_missing "$ORT_GPU_URL" "$ORT_GPU_NUPKG" 100000000

CUDART_ZIP="$CACHE_DIR/cuda_cudart-${CUDA_CUDART_VERSION}.zip"
CUDART_URL="https://developer.download.nvidia.com/compute/cuda/redist/cuda_cudart/windows-x86_64/cuda_cudart-windows-x86_64-${CUDA_CUDART_VERSION}-archive.zip"
download_if_missing "$CUDART_URL" "$CUDART_ZIP" 5000000

CUBLAS_ZIP="$CACHE_DIR/libcublas-${CUDA_CUBLAS_VERSION}.zip"
CUBLAS_URL="https://developer.download.nvidia.com/compute/cuda/redist/libcublas/windows-x86_64/libcublas-windows-x86_64-${CUDA_CUBLAS_VERSION}-archive.zip"
download_if_missing "$CUBLAS_URL" "$CUBLAS_ZIP" 200000000

CUDNN_ZIP="$CACHE_DIR/cudnn-${CUDNN_VERSION}.zip"
CUDNN_URL="https://developer.download.nvidia.com/compute/cudnn/redist/cudnn/windows-x86_64/cudnn-windows-x86_64-${CUDNN_VERSION}_${CUDNN_CUDA_TAG}-archive.zip"
download_if_missing "$CUDNN_URL" "$CUDNN_ZIP" 500000000

TRT_ZIP="$CACHE_DIR/TensorRT-${TRT_VERSION}.zip"
TRT_URL="https://developer.download.nvidia.com/compute/machine-learning/tensorrt/${TRT_VERSION%.*}/zip/TensorRT-${TRT_VERSION}.Windows.amd64.${TRT_CUDA_TAG}.zip"
download_if_missing "$TRT_URL" "$TRT_ZIP" 2000000000

# ───────────────────────────────────────────────
# 2. 展開と DLL 抽出
# ───────────────────────────────────────────────
echo ""
echo "═══════════════════════════════════════════════"
echo " Step 2/3: Extract DLLs to $TARGET_DIR"
echo "═══════════════════════════════════════════════"

# 既存 INSTALL_OK は最後に書き直すので削除
rm -f "$TARGET_DIR/INSTALL_OK"

# 既存の DLL は基本的に上書き
echo "  extracting ORT GPU NuGet..."
unzip -o -j "$ORT_GPU_NUPKG" \
    "runtimes/win-x64/native/onnxruntime.dll" \
    "runtimes/win-x64/native/onnxruntime_providers_cuda.dll" \
    "runtimes/win-x64/native/onnxruntime_providers_tensorrt.dll" \
    "runtimes/win-x64/native/onnxruntime_providers_shared.dll" \
    -d "$TARGET_DIR" > /dev/null

echo "  extracting CUDA cudart..."
unzip -o -j "$CUDART_ZIP" "*/bin/cudart64_*.dll" -d "$TARGET_DIR" > /dev/null

echo "  extracting CUDA cublas..."
unzip -o -j "$CUBLAS_ZIP" \
    "*/bin/cublas64_*.dll" \
    "*/bin/cublasLt64_*.dll" \
    -d "$TARGET_DIR" > /dev/null

echo "  extracting cuDNN..."
unzip -o -j "$CUDNN_ZIP" "*/bin/cudnn*.dll" -d "$TARGET_DIR" > /dev/null

echo "  extracting TensorRT..."
unzip -o -j "$TRT_ZIP" \
    "*/lib/nvinfer_*.dll" \
    "*/lib/nvinfer*.dll" \
    "*/lib/nvonnxparser_*.dll" \
    -d "$TARGET_DIR" > /dev/null 2>&1 || true
# TRT zip の内部構造はバージョンで変わるので、見つかった DLL 数を後でチェック

# ───────────────────────────────────────────────
# 3. INSTALL_OK sentinel 書き込み
# ───────────────────────────────────────────────
echo ""
echo "═══════════════════════════════════════════════"
echo " Step 3/3: Verify and write INSTALL_OK"
echo "═══════════════════════════════════════════════"

required_dlls=(
    "onnxruntime.dll"
    "onnxruntime_providers_tensorrt.dll"
    "onnxruntime_providers_cuda.dll"
    "onnxruntime_providers_shared.dll"
)
missing=()
for dll in "${required_dlls[@]}"; do
    if [[ ! -f "$TARGET_DIR/$dll" ]]; then
        missing+=("$dll")
    fi
done

# nvinfer 系は名前にバージョンが入るので glob でチェック
nvinfer_count=$(ls "$TARGET_DIR"/nvinfer_*.dll 2>/dev/null | wc -l)
cudart_count=$(ls "$TARGET_DIR"/cudart64_*.dll 2>/dev/null | wc -l)
cudnn_count=$(ls "$TARGET_DIR"/cudnn*.dll 2>/dev/null | wc -l)

if [[ ${#missing[@]} -gt 0 ]]; then
    echo "ERROR: 必須 DLL が不足: ${missing[*]}" >&2
    exit 1
fi
if (( nvinfer_count == 0 )); then
    echo "ERROR: nvinfer_*.dll が抽出されなかった (TRT zip 構造を確認)" >&2
    exit 1
fi
if (( cudart_count == 0 )); then
    echo "ERROR: cudart64_*.dll が抽出されなかった" >&2
    exit 1
fi
if (( cudnn_count == 0 )); then
    echo "ERROR: cudnn*.dll が抽出されなかった" >&2
    exit 1
fi

cat > "$TARGET_DIR/INSTALL_OK" <<EOF
{
  "version": 1,
  "ort_gpu_version": "${ORT_GPU_VERSION}",
  "cuda_cudart_version": "${CUDA_CUDART_VERSION}",
  "cuda_cublas_version": "${CUDA_CUBLAS_VERSION}",
  "cudnn_version": "${CUDNN_VERSION}",
  "trt_version": "${TRT_VERSION}",
  "installed_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF

total_size=$(du -sh "$TARGET_DIR" | cut -f1)
dll_count=$(ls "$TARGET_DIR"/*.dll 2>/dev/null | wc -l)

echo ""
echo "✓ Setup complete"
echo "  Target: $TARGET_DIR"
echo "  DLLs:   $dll_count files, total $total_size"
echo "  Sentinel: INSTALL_OK"
echo ""
echo "次のステップ: cargo run --release --features dev-tools --bin bench_ai -- --backend tensorrt"
