# AI 処理サイズしきい値 修正要件

> ステータス: **実装済み (2026-06-10)**。`final-smart-sharpen-plan.md` とは別管理にする。
>
> 実装サマリ:
>
> - `ai::upscale::AiProcessSizeLimit` + `should_process_rect` を追加し、旧
>   `should_process` の呼び出しを全て置き換えた (単体テストあり)。
> - `Settings::ai_upscale_size_limit` / `ai_denoise_size_limit` (`Option<AiProcessSizeLimit>`)
>   を追加。`None` (= 旧設定) は `Settings::ai_upscale_limit()` / `ai_denoise_limit()` が
>   旧 `*_skip_px` を `N x N` として読み替える。旧フィールドは downgrade 互換のため残置。
> - 環境設定 UI はラジオボタン → コンボボックス (`AI_SIZE_LIMIT_OPTIONS`、本書の候補 6 種)。
>   上限変更時は `App::apply_ai_size_limit_change` が final AI cache / failed / pending と
>   旧 AI cache を無効化する。
> - 4x 後の縮小は `run_final_ai_job` (AI worker) 内の `clamp_color_image_for_gpu`
>   (premultiplied RGBA8 のまま SIMD リサイズ) で実施。`final_ai_cache` には 8192px 超は
>   入らない。`output_is_ai_upscaled` のサイズ比較は縮小後も成立する (入力長辺 < 4096 <
>   縮小後長辺 8192) — 詳細は `docs/preset-and-adjustment.md` のスマートシャープ節。
> - 2x モデル追加・streaming downsample・ノイズ除去の原寸適用は本書記載どおり未実装。
> - 既知の制限 (Codex P2 指摘、実装前からの既存挙動): PDF ページの初回フルスクリーン
>   レンダは content_type 未解析のため 4096px 固定 (`start_fs_load`)。final AI はレンダ
>   後のピクセルサイズで判定するので、ラスターページでも初回表示では AI が掛からない
>   (旧しきい値 2048 でも同じ)。ズーム操作で `request_pdf_rerender` がサイズ上限内の
>   ラスターページを native 解像度で再レンダした時点で AI が効く。content_type 解析後の
>   自動再レンダ (= 初回表示から AI を効かせる) は PDF pool 負荷とのトレードオフがあり
>   本実装のスコープ外。

## 目的

AI アップスケール / AI ノイズ除去の適用サイズを、現在の単一しきい値から
「長辺 x 短辺」の上限指定へ拡張する。

5ch で要望があった「2048px を超える画像にも AI 処理を掛けたい」ケースに対応しつつ、
GPU テクスチャ上限 8192px と、4x アップスケール時の巨大な中間メモリを避ける。

## 現状

- `Settings::ai_upscale_skip_px` / `Settings::ai_denoise_skip_px` は `u32` の単一値。
- 既定値はどちらも `2048`。
- 環境設定 UI は `[512, 1024, 2048]` のラジオボタン。
- 判定は `ai::upscale::should_process(width, height, threshold)`。
- 現在の判定は `width < threshold && height < threshold`。
  つまり「幅または高さがしきい値以上ならスキップ」。
- 判定箇所は `final_ai_key_for_pixels` / `maybe_start_final_ai` /
  `is_idx_final_ai_done_or_skipped` / パノラマ source 判定 / 補正パネルの無効表示など複数ある。
  実装時は `rg "should_process"` で全呼び出しを確認する。

GPU 側は `MAX_TEXTURE_DIM = 8192`。`clamp_for_gpu` は 8192px 超の `ColorImage` を
UI スレッドで縮小する安全網だが、AI アップスケール後の大画像では重い。
4x 後に 8192px を超えるケースを許可するなら、UI スレッドの `clamp_for_gpu` に任せず、
AI worker 側または final AI 結果取り込み前に縮小しておく必要がある。

## 現行アップスケールモデル

現行 mIV の UI に出るアップスケールモデルはすべて 4x 系。

| `ModelKind` | ファイル | 倍率 | 備考 |
| --- | --- | --- | --- |
| `UpscaleRealEsrganX4Plus` | `realesrgan_x4plus.onnx` | 4x | 写真・CG |
| `UpscaleRealEsrganAnime6B` | `realesrgan_x4plus_anime_6b.onnx` | 4x | イラスト・アニメ |
| `UpscaleRealCugan4x` | `realcugan_4x_conservative.onnx` | 4x | 漫画・トーン保持 |
| `UpscaleNmkdSiax4x` | `4x_NMKD-Siax_200k.onnx` | 4x | 写真・質感保持 |
| `UpscaleRealEsrGeneralV3` | `realesr_general_x4v3.onnx` | 4x | 高速汎用 |

`models/waifu2x_cunet_noise3_scale2x.onnx` は loose file として残っているが、
`ModelKind` では廃止扱いで、`upscale_models()` / UI / 自動選択には入っていない。

## 新しいサイズ指定

単一値ではなく、正規化した「長辺 / 短辺」の上限で判定する。

```rust
pub struct AiProcessSizeLimit {
    pub long_edge_px: u32,
    pub short_edge_px: u32,
}

pub fn should_process_rect(width: u32, height: u32, limit: AiProcessSizeLimit) -> bool {
    let long = width.max(height);
    let short = width.min(height);
    long < limit.long_edge_px && short < limit.short_edge_px
}
```

既存の「しきい値以上ならスキップ」と合わせるため、初期実装では `<` 判定を維持する。
UI 表記も「4096 x 2048 未満」のようにしておくと境界値の誤解が少ない。

保存形式は次のいずれかにする。

- 推奨: `ai_upscale_size_limit` / `ai_denoise_size_limit` として構造体を追加する。
- 互換用に旧 `ai_upscale_skip_px` / `ai_denoise_skip_px` はすぐ削除しない。
- 新フィールドが無い設定ファイルでは、旧値 `N` を `N x N` として読み替える。

## UI 案

環境設定の「AI 処理のスキップしきい値」を、ラジオボタンからコンボボックスまたは
横幅を取らない選択 UI に変更する。

初期候補:

```text
512 x 512 未満
1024 x 1024 未満
2048 x 1024 未満
2048 x 2048 未満
4096 x 2048 未満
4096 x 4096 未満 (高負荷)
```

アップスケールは 4x 中間画像が大きくなるため、既定値は従来相当の
`2048 x 2048 未満` のままにする。

ノイズ除去は 1x なので理屈上は `8192 x 8192` まで扱えるが、推論時間とメモリ負荷が大きい。
初期 UI ではアップスケールと同じ候補に揃え、8192 系は必要なら後で上級者向け候補として追加する。

## 4x 後の縮小

現行アップスケールモデルはすべて 4x だが、将来 2x モデルを追加する可能性がある。
そのため「4x 決め打ち」ではなく、実際の出力サイズで GPU 上限を判定する。

要件:

- final AI 結果の `ColorImage` が `MAX_TEXTURE_DIM` を超える場合、長辺 8192px 以下に縮小する。
- 縮小は UI スレッドの `clamp_for_gpu` まで遅らせない。
- `final_ai_cache` / `final_composite_cache` に極端な 16K 級画像を保持しない。
- 縮小後も `post_filter` / シャープ化 / GPU upload の順序は保つ。

実装候補:

1. `run_final_ai_job` 内で、アップスケール成功直後の `ColorImage` を 8192px 以下へ縮小してから返す。
2. もしくは `FinalAiResult::Ready` 取り込み時に worker / background 側で縮小済みにする。

UI スレッドで `image::resize_exact` する経路は避ける。

## 将来の 2x モデル候補

大きい解像度の画像では、4x 全体出力を作ってから縮小するより、2x モデルで直接 2x 出力にした方が
中間メモリと処理時間を抑えられる可能性がある。

候補:

- `RealESRGAN_x2plus`
  - Real-ESRGAN 公式に存在する 2x 汎用モデル。
  - 現行 Real-ESRGAN 系と同じライセンス系統で扱いやすい。
  - 写真・CG向けの 2x 候補として優先度が高い。
- `Real-CUGAN 2x`
  - Real-CUGAN 公式に 2x / 3x / 4x がある。
  - 漫画・アニメ向けの 2x 候補。
  - 現行 `convert_realcugan_to_onnx.py` は 4x 専用なので、2x 用 wrapper を追加する必要がある。
- `waifu2x CUNet 2x`
  - 2x モデルは手元に残っているが、mIV では廃止扱い。
  - 追加するなら画質・速度・ライセンス表記を再評価する。

2x モデルを入れる場合は、単にモデルを増やすだけでなく、UI で「倍率」を明示する。
例: `漫画 2x (低負荷)` / `漫画 4x (高品質)`。
自動判別では、画像サイズが大きい場合は 2x、小さい場合は 4x のように選べる余地がある。

## メモリ目安

現行 `ai::upscale` は、出力サイズ全体に対して RGB 累積 3 面 + weight 1 面の `f32`
バッファを持ち、最後に `ColorImage` も作る。概算は出力 1 pixel あたり最低 20 bytes
前後、アルファや一時バッファを含めるとさらに増える。

例:

```text
1360 x 1920 を 4x        -> 5440 x 7680  = 41.8 MP  -> 約 0.8 GB 以上
2720 x 1920 を 4x        -> 10880 x 7680 = 83.6 MP  -> 約 1.7 GB 以上
2048 x 4096 を 4x        -> 8192 x 16384 = 134 MP   -> 約 2.7 GB 以上
4096 x 4096 を 4x        -> 16384 x 16384 = 268 MP  -> 約 5.3 GB 以上
```

8192px へ縮小して表示する場合でも、現行方式では一度 4x の全体出力を作る。
そのため `4096 x 4096 未満` は高スペック向けの上限として扱い、既定値にはしない。

将来さらに大きい入力を扱うなら、4x 全体出力を作らず縮小後サイズへ直接合成する
streaming downsample が必要になるが、今回の範囲外。

## 8192px 上限について

`MAX_TEXTURE_DIM = 8192` は wgpu / eframe の現在の初期化制約に合わせた上限。
実 GPU には 16384px 以上を扱えるものもあるが、アプリ側の wgpu limits、古い GPU /
ドライバ、描画コード、キャッシュ、分割テクスチャ対応まで巻き込むため、単純に上限を上げない。

5ch などで説明する場合は、
「互換性のため GPU テクスチャは 1 辺 8192px を上限にしています。
それ以上は分割描画が必要になり実装がかなり複雑になるため、当面は 4x 後に 8192px 以下へ
縮小する方向で対応します」
程度が安全。

## 実装箇所

- `src/settings.rs`
  - 新しいサイズ上限フィールドと default を追加。
  - 旧 `*_skip_px` からの移行を用意。
- `src/ui_dialogs/preferences/pages.rs`
  - AI 処理しきい値 UI を長辺 x 短辺の候補選択へ変更。
- `src/ai/upscale.rs`
  - `should_process_rect` を追加し、単体テストを追加。
  - 必要なら `ColorImage` を 8192px 以下へ縮小する helper を追加。
- `src/app.rs`
  - `should_process` 呼び出しをすべて新判定へ置き換える。
  - final AI 結果が 8192px 超のまま cache / texture upload へ流れないようにする。
- `src/ui_adjustment_panel.rs`
  - 「しきい値以上なので AI 無効」表示を新しい長辺 x 短辺表記へ変更。

## テスト観点

- 旧設定 `ai_upscale_skip_px = 2048` は新設定 `2048 x 2048 未満` と同じ挙動になる。
- `2720 x 1920` は `2048 x 2048 未満` では対象外、`4096 x 2048 未満` では対象になる。
- `1920 x 2720` のように縦横が逆でも同じ判定になる。
- アップスケール後の final AI 結果は、幅・高さとも `MAX_TEXTURE_DIM` 以下になる。
- AI 無効表示、先読み判定、パノラマ source 判定、final AI cache key 判定が同じ条件を使う。
- しきい値変更時に final AI cache / failed / pending が適切に無効化される。
- 4096 系を選んでも、処理開始前に明らかなメモリ過大ケースでクラッシュしない。

## スコープ外

- GPU テクスチャ上限そのものを 8192px より上げる。
- 8192px 超画像の分割テクスチャ描画。
- 4x 全体出力を作らない streaming downsample。
- ノイズ除去を、フルスクリーン表示用に 8192px 以下へ縮小される前の原寸画像へ掛ける処理。
