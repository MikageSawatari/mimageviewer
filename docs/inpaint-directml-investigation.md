# LaMa Inpainting × DirectML 互換性調査

## 目的

見開きスキャン画像の綴じ部分（gap）を AI で補完する機能で、GPU (DirectML) による高速推論を実現したい。

## 現状

### 動作環境
- Windows 11 Pro, RTX 4090
- ONNX Runtime 2.0 (ort crate) + DirectML EP
- Rust アプリケーション（eframe/egui ベース画像ビューワ）

### LaMa モデルの状況

| モデル | サイズ | 入力形状 | CPU | DirectML | 備考 |
|--------|--------|----------|-----|----------|------|
| lama_fp32.onnx (Carve) | 208MB | 固定 512×512 | OK (1.6s) | NG | FP32, opset 17 |
| inpainting_lama_2025jan.onnx (OpenCV) | 93MB | 固定 512×512 | OK (1.6s) | NG | Quantized |
| lama_dynamic_fp32.onnx (自前エクスポート) | 208MB | 動的 H×W | OK (12s@576×3008) | NG | FP32, opset 17, Carve fork 使用 |

### DirectML エラーの詳細

全モデル共通で以下のエラーが発生:

```
Non-zero status code returned while running MatMul node.
Name: '/generator/model/model.5/conv1/ffc/convg2g/fu/rttn/MatMul_5'
Status Message: ...MLOperatorAuthorImpl.cpp(2508)...
Exception(3) 80070057 パラメーターが間違っています。
```

- エラーは **FFC (Fast Fourier Convolution) 層** 内の **FourierUnit** で発生
- 具体的には `convg2g` (global-to-global convolution) の `fu` (FourierUnit) 内の `rttn` (FFT 代替の MatMul 実装)
- Carve fork の `FourierUnitJIT` は `torch.fft.rfftn` を MatMul ベースの実装に置換してONNX互換にしたが、DirectML の MatMul 実装との互換性問題がある
- HRESULT `0x80070057` = `E_INVALIDARG` (無効なパラメータ)

### FourierUnit の構造

LaMa の核心技術 FFC は以下の構造:

```
FFCResNetGenerator
  └── FFCResnetBlock (×9)
        ├── conv1: FFC_BN_ACT
        │     ├── convl2l (local→local): 通常の Conv2d ← DirectML OK
        │     ├── convl2g (local→global): 通常の Conv2d ← DirectML OK
        │     ├── convg2l (global→local): 通常の Conv2d ← DirectML OK
        │     └── convg2g (global→global): SpectralTransform ← ★ここが問題
        │           └── FourierUnit
        │                 └── rttn: MatMul ベースの FFT/iFFT ← DirectML NG
        └── conv2: (同構造)
```

FourierUnit は入力テンソルに対して FFT を実行し、周波数領域で畳み込みを行う。
ONNX では `torch.fft.rfftn` が直接エクスポートできないため、Carve fork は手動で MatMul ベースの DFT を実装 (`FourierUnitJIT`)。
この MatMul の形状/ストライドが DirectML の期待と合わない。

### DirectML で試していない設定

ONNX Runtime の DirectML EP には以下のセッション設定が推奨されている:

1. **`DisableMemPattern`** — メモリパターン最適化を無効化
2. **`ORT_SEQUENTIAL` 実行モード** — 逐次実行モード

```python
# Python での設定例（Rust/ort crate での設定方法は要調査）
opts = ort.SessionOptions()
opts.enable_mem_pattern = False
opts.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
```

これらを設定しても MatMul ノード自体の互換性問題は解決しない可能性が高いが、未検証。

### 試した・調べたこと

1. ✅ LaMa FP32 (Carve) — DirectML NG, CPU OK
2. ✅ LaMa Quantized (OpenCV) — DirectML NG, CPU OK（同じ MatMul エラー）
3. ✅ 動的サイズ LaMa を Carve fork から再エクスポート — DirectML NG, CPU OK
4. ✅ 512×512 固定で CPU 動作確認 — 1.6秒/回
5. ✅ 動的サイズで CPU 動作確認 — 576×3008 で 12秒
6. ❌ FP16 変換 — 未実施
7. ❌ Opset バージョン変更 (15, 16) — 未実施
8. ❌ DisableMemPattern / ORT_SEQUENTIAL — 未実施
9. ❌ ONNX グラフの MatMul ノードを手動修正 — 未実施
10. ❌ 問題の MatMul ノードだけ CPU にフォールバック — ONNX Runtime ではセッション単位のため困難

## 考えられる解決策

### A. LaMa を DirectML で動かす方向

1. **FP16 変換**: `onnxconverter-common` の `convert_float_to_float16` でモデルを FP16 に変換し、DirectML の別コードパスに乗せる
2. **Opset ダウングレード**: Opset 15 でエクスポートし直す
3. **MatMul ノードの修正**: ONNX グラフエディタ (onnx-modifier 等) で問題の MatMul の入力形状を調整
4. **FourierUnit を別実装に置換**: FFT 部分を ONNX の DFT op (opset 20+) に置換し、DirectML の DFT サポートに賭ける
5. **DirectML の memory pattern / execution mode 設定**: 効果は不明だが試す価値あり

### B. 別モデルに切り替える方向

| モデル | 特徴 | DirectML 互換性見込み | サイズ |
|--------|------|---------------------|--------|
| DeepFillv2 | Gated Conv ベース、FFC なし | 高（Conv のみ） | ~25MB |
| MI-GAN | 軽量、モバイル向け | 高（Conv ベース） | ~80MB |
| AOT-GAN | Conv + Attention | 中（標準的な構造） | 不明 |
| MST Inpainting | ONNX 実績あり | 中〜高 | 不明 |

- FFC 層を使わないモデルなら DirectML で動く可能性が高い
- 品質は LaMa より劣る可能性があるが、見開き gap の補完用途なら許容範囲かもしれない

### C. ハイブリッドアプローチ

- 高品質: LaMa Dynamic (CPU, 12秒) — 初回表示時や明示的な再処理時
- 高速プレビュー: 別モデル (DirectML, <1秒) — ドラッグ調整中のプレビュー

## 再現手順

### テストプログラム

```bash
# ベンチマーク（PDF の特定ページで比較）
cargo run --release --bin bench_inpaint -- "path/to/book.pdf" 77 78 40 5

# LaMa 単体テスト（各種サイズ + CPU/DirectML）
cargo run --release --bin test_inpaint
```

### モデルファイルの場所

```
%APPDATA%\mimageviewer\models\
  ├── lama_fp32.onnx                    # Carve 版 (512 固定)
  ├── lama_dynamic_fp32.onnx            # 自前エクスポート (動的)
  └── inpainting_lama_2025jan.onnx      # OpenCV 版 (512 固定)
```

### 動的 LaMa ONNX の再エクスポート

```bash
cd H:/home/lama-carve
python export_dynamic.py
# → lama_dynamic_fp32.onnx (208MB)
```

依存: `torch`, `omegaconf`, `pyyaml`, `onnxruntime`
チェックポイント: `H:/home/lama/big-lama/models/best.ckpt`

## 参考リンク

- [Carve/LaMa-ONNX](https://huggingface.co/Carve/LaMa-ONNX) — FP32 固定 512×512 版
- [opencv/inpainting_lama](https://huggingface.co/opencv/inpainting_lama) — OpenCV Quantized 版
- [advimman/lama](https://github.com/advimman/lama) — LaMa 公式リポジトリ
- [Carve-Photos/lama](https://github.com/Carve-Photos/lama) — ONNX エクスポート対応 fork
- [ONNX Runtime DirectML EP ドキュメント](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html)
- [ford442/deepfillv2-inpainting](https://huggingface.co/ford442/deepfillv2-inpainting) — DeepFillv2 ONNX
- [Picsart-AI-Research/MI-GAN](https://github.com/Picsart-AI-Research/MI-GAN) — MI-GAN
