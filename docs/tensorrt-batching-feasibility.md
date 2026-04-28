# 明示バッチング (Explicit Batching) 実装可能性メモ

mimageviewer の AI アップスケールで TensorRT に乗り換えた後、更なる高速化候補として
「複数タイルを 1 推論にまとめる」明示バッチングを検討した際のメモ。

## 現状のパイプライン (`src/ai/upscale.rs`)

```
[Main Thread (推論)]            [Blender Thread]
extract_tile (CPU, 0.12 ms)
run_tile_inference:
  tensor_build (CPU)             ← 入力テンソル構築
  session.run() ← GPU 11-25 ms   blend_tile (CPU, ~3 ms)
  tensor_extract (CPU)
  post_copy (CPU)
sync_channel.send() ────→  rx.recv()
```

- `sync_channel(2)` で背圧、推論と blend が 1 タイル分オーバーラップ
- 1 タイル 1 推論 (batch size = 1)
- per-tile breakdown (TRT, anime6b, 256x256 tile, 4x → 1024x1024 出力):
  - extract: 0.12 ms (1%)
  - infer: 11.45 ms (76%, うち session_run 9-10 ms)
  - blend: 3 ms (20%)

## 期待される効果

GPU 推論部分が総時間の 60-90% を占めるので、**バッチ化で GPU 部を 25-30% 削減できれば
総合 1.25-1.4x の追加高速化**が見込める。例えば:

- 現在: 290.6 ms (mistblossom 896x1152, anime6b, 20 tiles)
- バッチ 4: 232 ms × 0.7 + 58 ms = 220 ms 程度 (1.32x)

## 実装の必要作業

### 1. バッチサイズの決定とゲーティング
- DirectML: バッチ 1 のまま (DirectML の per-call overhead は batch でほぼ変わらず)
- TensorRT: バッチ 4 推奨 (8 だと VRAM 圧迫が懸念、固定タイル 256 + 4x = 出力 1024x1024 RGBA で 16 MiB × 4 = 64 MiB、複数モデル warmup 想定で計 1 GiB)
- CPU: バッチ 1 (バッチ化の旨味なし)

### 2. `upscale.rs` の構造変更
- `extract_tile` を N 回呼んで `Vec<Array4<f32>>` を作る
- バッチ次元 N で stack → `Array4` shape `(N, 3, H, W)` のテンソル
- `run_tile_inference_batch` (新規) で 1 回 `session.run()`
- 出力テンソル shape `(N, 3, H*scale, W*scale)` を N 個の TileOutput に split
- N 個まとめて `tx.send` (またはループで N 回送る)
- 端数バッチ (例: 全 20 タイル / batch 4 = 5 バッチ) は割り切れる
- 端数発生時は 0 パディングで最後のバッチを満たすか、batch 1 の旧経路にフォールバック

### 3. TensorRT エンジンとの相性
- TRT エンジンは shape ごとにビルド/キャッシュされる
- 静的バッチ batch=4 のエンジンは batch=1 用に使えない (逆も)
- 二択:
  - (A) 動的バッチ (最小 1 / 最大 4) のエンジン → 5-15% 遅くなる
  - (B) batch=4 用エンジンと batch=1 用 (端数用) エンジンを別々にビルドキャッシュ
- **推奨: (B)** — エンジンキャッシュサイズは増えるが、定常状態が速い

### 4. キャンセル粒度の悪化
- 現在: 1 タイルごとに `cancel.load()` チェックでキャンセル可能
- バッチ後: バッチ単位でしかチェックできない (バッチ 4 = 約 30 ms のレイテンシ)
- フルスクリーン表示の Ctrl+→ 連打等で UI 応答性に影響しないか要確認

## 実装規模見積もり

- `upscale.rs`: ~100 行追加 (バッチ版 inference + テンソル split)
- `runtime.rs`: TRT EP に動的バッチプロファイル設定 (5 行)
- 既存テスト維持: tile_blend テストは batch=1 の前提なので、バッチサイズ既定値の挙動を分けて修正
- bench: `--batch-size N` フラグ追加で実験可能に

合計、実装 + テスト + 検証で **2-3 時間程度**。

## 判断: 後回し

- **現在の 1.55-1.80x (アップスケール) / 3.93x (デノイズ) は十分大きい**
- 明示バッチングは **静音環境での最終測定で現状の数字が確定してから** 着手判断
- 仮に 1.3x 追加なら大画像の wall total が 1719 → 1320 ms 程度になり魅力的だが、
  キャンセル応答性の悪化と引き換え
- **デノイズ (RealPLKSR) は固定入力 256x256 なのでバッチ化と相性最良**、
  ここから着手するのが筋

## 関連ファイル

- `src/ai/upscale.rs` の `upscale_with_timings`, `run_tile_inference`
- `src/ai/runtime.rs` の `register_tensorrt_eps`
