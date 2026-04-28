mImageViewer (mIV) で **TensorRT 高速化機能** を有効化したときに自動的にダウンロード
されるパックです。本リリースの zip / DLL を **手動でダウンロードする必要はありません** —
mIV の環境設定で「TensorRT」を選択し「TensorRT パックをダウンロード」を押すと、
本ページのアセットが自動取得されます。

## 構成

- **ONNX Runtime 1.24.2** (GPU 版) - Microsoft, MIT License
- **CUDA Runtime / Math / NVRTC / nvJitLink** (CUDA Toolkit 12.9 の必要最小 DLL)
  - `cudart64_12.dll`, `cublasLt64_12.dll`, `cufft64_11.dll`, `nvJitLink_120_0.dll`,
    `nvrtc64_120_0.dll`, `nvrtc-builtins64_129.dll`
- **cuDNN 9.21** - `cudnn_ops64_9.dll` のみ
- **TensorRT 10.16 runtime** - `nvinfer_10.dll` + `nvinfer_plugin_10.dll`
- **6 モデル分の事前ビルド済み engine** (`engines-ampere_plus.zip`)
  - Real-ESRGAN x4plus / anime6b / general_v3
  - Real-CUGAN 4x
  - NMKD-Siax 4x
  - RealPLKSR (デノイズ)
  - AMPERE_PLUS hardware-compatible モード (sm80+ で動作)

各ファイルの SHA-256 と総バイト数は `manifest.json` に記載されており、mIV
ダウンローダーが各アセットの整合性を検証します。

## 容量・所要時間

- 合計ダウンロード量: **約 1.97 GB**
- 所要時間目安: 5〜15 分 (ネットワーク速度による)
- 中断・再開対応 (HTTP Range)

## 対応 GPU

| GPU 世代 | 対応 |
|---|---|
| RTX 50 シリーズ (Blackwell, sm120) | ✓ |
| RTX 40 シリーズ (Ada, sm89) | ✓ |
| RTX 30 シリーズ (Ampere, sm86) / A100 (sm80) | ✓ |
| RTX 20 シリーズ / GTX 16 シリーズ (Turing, sm75) | ✗ (DirectML フォールバック) |
| GTX 10 シリーズ以前 | ✗ |

## ライセンス

同梱コンポーネントの再配布は以下の条件下で行われています。詳細はパック内の
`NOTICE-NVIDIA.txt` および `LICENSE-onnxruntime.txt` を参照してください。

- **NVIDIA コンポーネント**: NVIDIA Software License Agreement for SDKs +
  CUDA Toolkit EULA / cuDNN SLA / TensorRT SLA に準拠して、mImageViewer 専用に
  再配布されています。**抽出・別アプリでの再利用・別途再配布は禁止**されます。
- **ONNX Runtime**: MIT License (Microsoft Corporation)

## 検証 (manifest.json)

```bash
# manifest.json を読み取って各ファイルの sha256 を確認
curl -fsSL https://github.com/MikageSawatari/mimageviewer/releases/download/trt-pack-v1/manifest.json | jq .
```
