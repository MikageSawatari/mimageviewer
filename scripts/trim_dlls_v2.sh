#!/usr/bin/env bash
# v2 DLL trim test:
# **TRT EP が実際に使われていること** を確認しながら最小 DLL セットを再決定する。
#
# 前回 (v1 trim) は `bench_ai --runs 1` の wall total emit だけで成功判定していたが、
# ORT は TRT EP load 失敗時に CUDA → CPU と silent fallback するので、CPU EP で完走
# しても "成功" に見えてしまっていた。実機リプレイで判明した crash の根本原因。
#
# v2 では以下の二段階で判定する:
#   1. bench_ai が --backend tensorrt で完走 (= worker process が crash しない)
#   2. 推論の **session_run min < 200ms** (= TRT 経路、CUDA/CPU fallback ではない)
#      TRT で 512^2 tile ~150ms、256^2 tile ~85ms。CUDA は 200-500ms。CPU は 1500ms+。
#
# 実行手順:
#   1. setup-tensorrt-pack.ps1 でフル pack を %APPDATA%/mimageviewer/tensorrt/ に展開
#   2. mimageviewer.exe --tensorrt-build <kind> で 6 モデル分の engine を pre-build
#   3. このスクリプトを実行 (~15-30 分)
#
# 出力:
#   /tmp/trim_dlls_v2/result.txt   <- REQUIRED / REMOVABLE 一覧 (gold standard)
#   /tmp/trim_dlls_v2/m_*.log      <- 各テストの bench_ai 出力

set -euo pipefail

PACK_DIR="C:/Users/mikag/AppData/Roaming/mimageviewer/tensorrt"
PARK_DIR="C:/Users/mikag/AppData/Roaming/mimageviewer/tensorrt.parked"
TEST_IMG="C:/Users/mikag/AppData/Local/Temp/bench_800x600.png"
BENCH_EXE="/c/home/mimageviewer/.claude/worktrees/dazzling-mcclintock-d0be64/target/release/bench_ai.exe"
LOG_DIR="/tmp/trim_dlls_v2"
mkdir -p "$LOG_DIR" "$PARK_DIR"

# 6 モデル全てで session_run < SESSION_RUN_MAX_MS であることを要求する。
# 200 ms は TRT (~85-150ms) と CUDA EP (~200-500ms) を分ける典型値。
# 実測で TRT 上限が 180ms 付近のモデル (x4plus 大 tile 等) にぶつかったら緩める。
SESSION_RUN_MAX_MS=200

# DLL 候補リスト。前回 REMOVABLE 判定された 27 個から、hotfix で必須と判明した
# 3 個 (cublas64_12, cudnn64_9, nvonnxparser_10) を除外。
# 順序: 「絶対不要そう」→「グレー」→「builder_resource (法的に外したい)」。
CANDIDATES=(
  # math 系 (TRT/CUDA EP では未使用が定説)
  cufftw64_11.dll
  curand64_10.dll
  cusolver64_11.dll
  cusolverMg64_11.dll
  cusparse64_12.dll
  # cuDNN 補助 (TRT は AMPERE_PLUS で cuDNN tactic を使わない)
  cudnn_adv64_9.dll
  cudnn_cnn64_9.dll
  cudnn_engines_precompiled64_9.dll
  cudnn_engines_runtime_compiled64_9.dll
  cudnn_engines_tensor_ir64_9.dll
  cudnn_graph64_9.dll
  cudnn_heuristic64_9.dll
  # TRT 補助 (バージョン互換 lib)
  nvinfer_lean_10.dll
  nvinfer_dispatch_10.dll
  nvinfer_vc_plugin_10.dll
  # NVRTC alt
  nvrtc64_120_0.alt.dll
  # builder_resource (法的に外したい、事前 build engine で代替予定)
  nvinfer_builder_resource_ptx_10.dll
  nvinfer_builder_resource_sm75_10.dll
  nvinfer_builder_resource_sm80_10.dll
  nvinfer_builder_resource_sm86_10.dll
  nvinfer_builder_resource_sm89_10.dll
  nvinfer_builder_resource_sm90_10.dll
  nvinfer_builder_resource_sm100_10.dll
  nvinfer_builder_resource_sm120_10.dll
)

# ---- helpers ----
park_dll() {
  local name="$1"
  if [[ -f "$PACK_DIR/$name" ]]; then
    mv "$PACK_DIR/$name" "$PARK_DIR/$name"
    return 0
  fi
  return 1
}

restore_dll() {
  local name="$1"
  if [[ -f "$PARK_DIR/$name" ]]; then
    mv "$PARK_DIR/$name" "$PACK_DIR/$name"
    return 0
  fi
  return 1
}

# 全 6 モデルを TRT で走らせ、session_run min が閾値以下か判定。
# 戻り値: 0 = OK (= TRT で動いた)、1 = NG。
test_trt_works() {
  local label="$1"
  local logfile="$LOG_DIR/m_${label}.log"

  if ! "$BENCH_EXE" --image "$TEST_IMG" \
       --models realesrgan_x4plus,realesrgan_anime6b,realesr_general_v3,realcugan_4x,nmkd_siax_4x,denoise_realplksr \
       --backend tensorrt --warmup 1 --runs 1 \
       > "$logfile" 2>&1; then
    echo "    NG: bench_ai exit non-zero (log: $logfile)"
    return 1
  fi

  # skip 行があったら NG (= 1 モデルでも crash した)
  if grep -qE "^  skip \[" "$logfile"; then
    echo "    NG: 1 モデル以上で skip (worker crash の可能性)"
    grep "^  skip" "$logfile" | head -3 | sed 's/^/      /'
    return 1
  fi

  # 全 6 モデルで wall total が出ているか
  local wall_count
  wall_count=$(grep -c "wall total" "$logfile" || true)
  if [[ "$wall_count" -ne 6 ]]; then
    echo "    NG: wall total が ${wall_count}/6 個 (不完全)"
    return 1
  fi

  # session_run min を全モデルから抽出して、最大値が SESSION_RUN_MAX_MS 以下か検証。
  # 形式例: 「infer:   88.78 ms ( 94.1%)   [min  20.69 / median  21.73 / max 828.05]」
  # の min を取り出す。
  local max_min_session
  # Sometime "infer:" 行に min/median/max が並ぶ。"min" 直後の数値を抽出。
  max_min_session=$(grep -E "infer:" "$logfile" \
    | grep -oE "min\s+[0-9]+\.[0-9]+" \
    | awk '{print $2}' \
    | sort -g \
    | tail -1)
  if [[ -z "$max_min_session" ]]; then
    echo "    NG: infer min を抽出できなかった (log format 異常?)"
    return 1
  fi
  # bash で float 比較は bc で
  local pass
  pass=$(awk -v a="$max_min_session" -v b="$SESSION_RUN_MAX_MS" \
    'BEGIN { print (a < b) ? 1 : 0 }')
  if [[ "$pass" -ne 1 ]]; then
    echo "    NG: 最大 infer min = ${max_min_session} ms (TRT なら < ${SESSION_RUN_MAX_MS} ms)"
    echo "      → CUDA/CPU EP fallback の疑い"
    return 1
  fi
  echo "    OK: 最大 infer min = ${max_min_session} ms (< ${SESSION_RUN_MAX_MS} ms、TRT 動作)"
  return 0
}

trim_one() {
  local name="$1"
  if [[ ! -f "$PACK_DIR/$name" ]]; then
    echo "  skip (not present): $name"
    return
  fi
  echo "[try removing] $name"
  if park_dll "$name"; then
    if test_trt_works "$name"; then
      echo "  REMOVABLE: $name"
      echo "REMOVABLE: $name" >> "$LOG_DIR/result.txt"
    else
      echo "  REQUIRED: $name (復元)"
      echo "REQUIRED: $name" >> "$LOG_DIR/result.txt"
      restore_dll "$name"
    fi
  fi
}

# ---- main ----
echo "=== v2 trim test 開始 ==="
echo "PACK_DIR : $PACK_DIR"
echo "LOG_DIR  : $LOG_DIR"
echo "閾値     : session_run min < ${SESSION_RUN_MAX_MS} ms"
echo

# baseline: フル DLL で TRT が動くか先に確認
echo "[baseline] フル DLL で TRT が動くこと"
if ! test_trt_works "baseline"; then
  echo "ERROR: baseline (フル DLL) で TRT が動かない。テストハーネスがおかしい。"
  echo "  bench_ai のバイナリパスや engine cache を確認してください。"
  exit 1
fi
echo

> "$LOG_DIR/result.txt"

for dll in "${CANDIDATES[@]}"; do
  trim_one "$dll"
done

echo
echo "=== 結果サマリ ==="
cat "$LOG_DIR/result.txt"
echo
echo "=== 残存 DLL (= 必須セット) ==="
ls "$PACK_DIR/" | grep -E "\.dll$" | sort
echo
TOTAL=$(du -sb "$PACK_DIR" | awk '{print $1}')
printf "total: %s bytes (%.2f GB)\n" "$TOTAL" \
  "$(awk -v t="$TOTAL" 'BEGIN { print t/1073741824 }')"
echo
echo "park された DLL (= REMOVABLE 一覧):"
ls "$PARK_DIR/" 2>/dev/null | sort
