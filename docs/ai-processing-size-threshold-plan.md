# AI 処理サイズしきい値 修正要件

> ステータス: **実装済み (2026-06-10)**。render-to-target 最適化は 2026-06-15 に実装。
> `archive/editing/final-smart-sharpen-plan.md` とは別管理にする。
>
> 実装サマリ:
>
> - `ai::upscale::AiProcessSizeLimit` + `should_process_rect` を追加し、旧
>   `should_process` の呼び出しを全て置き換えた (単体テストあり)。
> - `Settings::ai_upscale_size_limit` / `ai_denoise_size_limit` (`Option<AiProcessSizeLimit>`)
>   を追加。`None` (= 旧設定) は `Settings::ai_upscale_limit()` / `ai_denoise_limit()` が
>   旧 `*_skip_px` を `N x N` として読み替える。旧フィールドは downgrade 互換のため残置。
> - 環境設定 UI はラジオボタン → コンボボックス (`AI_SIZE_LIMIT_OPTIONS`)。
>   上限変更時は `App::apply_ai_size_limit_change` が final AI cache / failed / pending と
>   旧 AI cache を無効化する。
> - final pipeline のアップスケールは 4x 全面 `ColorImage` を作らず、upscaler が
>   `MAX_TEXTURE_DIM` 以下の target へタイルを直接合成する。`clamp_color_image_for_gpu` は
>   安全網として残す。`final_ai_cache` entry は pixels と `used_upscale` を持ち、
>   smart sharpen のスキップ判定はサイズ比較ではなく `used_upscale` を使う。
> - 2x モデル追加・ノイズ除去の原寸適用は本書記載どおり未実装。
> - **(旧・既知の制限 → v1.3.1 で解消、GitHub issue #1)**: PDF ページの初回フルスクリーン
>   レンダは content_type 未解析のため 4096px 固定 (`start_fs_load`)。final AI はレンダ
>   後のピクセルサイズで判定するので、ラスターページでも初回表示では AI が掛からなかった
>   (旧しきい値 2048 でも同じ。Codex P2 指摘、実装前からの既存挙動)。当初はズーム操作で
>   `request_pdf_rerender` がサイズ上限内のラスターページを native 解像度で再レンダした
>   時点でしか AI が効かず、「content_type 解析後の自動再レンダは PDF pool 負荷との
>   トレードオフがありスコープ外」としていた。**v1.3.1 で毎フレームの reconcile を追加**:
>   `App::update` の `maybe_native_rerender_pdf_for_ai` が、対象ページが「native は
>   サイズ上限内なのに現行レンダ寸法が native より十分大きい」なら
>   `request_pdf_rerender(idx, 1.0)` で native へ再レンダする。content_type 到着時
>   (初回表示で AI ON) に加え、**開いてサイズ確認後に U/N キーで AI を ON にする
>   issue 本文のシナリオ**も毎フレーム評価で拾う。
>   - **対象ページ範囲 (Codex 再レビュー P1)**: fs_idx だけでなく **見開き相方ページ +
>     連続表示の可視ページ (`fs_vertical_cache_keep_set`)** にも適用する。これらも
>     `ui_fullscreen` が `resolve_fs_processed_texture` で final AI をかけるため、片側だけ
>     native 化すると相方が 4096px のまま AI skip になる。AI 有効判定は cache 済み
>     `ai_upscale_enabled` ではなく **ページ個別 `effective_params(idx)` →
>     `effective_upscale_request` / `effective_denoise_request`** (= `maybe_start_final_ai`
>     と同じ) で行うので、ページ個別 AI 設定が異なる相方ページでも final AI ゲートと一致する。
>   - **高負荷上限でも native へ落とす (Codex 再レビュー P2)**: 条件は
>     `ai_at_native && cur_long > native_long * 1.1`。`!ai_at_cur` ではないので、上限を
>     `4096` にして現行 4096px レンダ (例 2812×4095 ≈ 11.5MP) も「AI 対象」になるケースでも、
>     より小さい native (例 824×1200 ≈ 1MP) へ落としてから AI する。PDFium の補間アップ
>     スケール出力を AI に流すより軽く・高品質。`1.1` 倍は `request_pdf_rerender` の dedup と
>     一致し、収束後 (cur≈native) は false で毎フレーム呼んでもループ・無駄打ちなし。
>   判定は純関数 `ai::upscale::pdf_needs_native_rerender_for_ai` (in-flight 中 / ズーム中 /
>   収束後は false。単体テスト + app 配線テストあり)。fit 表示 (zoom≈1.0) のみ・1 ページ
>   1 回 (収束) なので pool 負荷は限定的。非 AI 利用時 (AI 設定 OFF → `ai_at_native` が false)
>   は再レンダせず 4096px 表示を維持する (range 内ラスターを常に native へ落とすと表示解像度が
>   下がるため「設定 ON」を AND 条件にしている)。
>   - **AI 先読みを画像と同等にする (Codex 再々レビュー P2)**: 非表示の先読みページ
>     (`ai_prefetch_targets`) も Raster PDF なら、`prefetch_final_ai` が
>     `pdf_prefetch_should_defer_ai` で「現行 4096px に AI を流しても表示時 native 再レンダで
>     捨てる」状態を検出し、**native 再レンダを通常レーンで先に起動 + AI 先読みを native
>     着地まで保留**する。これで移動時に native + final AI が揃って画像同様に即表示できる
>     (4096px への無駄 AI も回避)。`pdf_prefetch_should_defer_ai` は visible 判定と違い
>     **ズーム / in-flight を見ない** (先読みページは fit で開かれる前提、再レンダ中も保留
>     継続)。native 再レンダの優先度: `request_pdf_rerender(idx, zoom, priority)` で
>     **visible/ズーム = priority レーン、先読み = 通常レーン** に振り分け (PDF worker は
>     priority を先に drain するので、先読みの native 再レンダが visible の再レンダや UI
>     ナビ enumerate を妨げない)。トレードオフ: 初回パスでネイバーは 4096px→native の 2 回
>     レンダ (= 画像が neighbor を decode する分の PDF 版。通常レーン・小サイズ・1 回収束)。
>   - **表示経路でも 4096px AI を保留 (Codex 4th P2)**: 非表示先読みだけでなく、表示中ページ
>     でも native 再レンダ待ち/進行中は捨てる 4096px (高負荷上限で ~11.5MP) final AI を
>     流さない。`ensure_final_composite_texture` が `display_should_defer_final_ai` で保留し、
>     暫定 (color-adjusted) を complete=false で見せて native 着地後に AI 起動する。保留時は
>     native 再レンダを priority レーンで保証起動 (reconcile 対象外経路でのハング防止、
>     in-flight 中は二重起動しない)。KICK / 表示保留 / 先読み保留の 3 判定は
>     `App::pdf_native_downscale_pending(idx, respect_zoom, gate_in_flight)` に統一
>     ((t,t)/(t,f)/(f,f))。中核数式は純関数 `ai::upscale::pdf_render_exceeds_native_ai_target`。
>   - **256px clamp の収束 (Codex 4th P2)**: `request_pdf_rerender` は `target_px` を
>     `[PDF_RENDER_MIN_LONG_PX(=256), PDF_RENDER_MAX_LONG_PX(=8192)]` に clamp するので、
>     native 100×200 の実レンダ着地は長辺 256 (128×256)。収束判定を raw `native_long(200)`
>     で見ると `256 > 200×1.1` で永久に「再レンダ要/AI 保留」のままになり、極小 Raster PDF で
>     先読みが効かなくなる。純関数に `render_min_long` を渡し **実 target 長辺
>     `max(native_long, 256)`** と比較して収束させる (テスト `min_clamp_lets_tiny_native_converge`)。
>   - **スコープ**: 対象は `PdfPageContentType::Raster` のページ。可視テキスト / パス /
>     シェーディングを含むページは `Vector` 判定 (`pdf_loader::analyze_page_content`) で
>     4096px 固定のまま AI 対象外 (透明 OCR テキストは `is_visible_text` が無視するので
>     スキャン PDF は Raster のまま対象)。ベクター混在ページの AI 化は別途要検討。
> - **メモリ過大ガードは意図的に持たない** (2026-06-10 ユーザー判断、Codex P2 への回答):
>   空きメモリ量に応じて AI 適用可否を変えると挙動が予測できなくなるため、判定は
>   サイズ上限のみで決定的にする。旧方式では 4x 全面出力の累積 f32 4 面 + 最終
>   ColorImage がピークを押し上げたが、現行の render-to-target では合成バッファを
>   最終 target (最大 8192x8192) に抑える。高負荷上限を明示的に選んだ環境で
>   実メモリが不足した場合のクラッシュは許容仕様。`try_reserve` での graceful fail も
>   「メモリ次第で AI が掛かったり掛からなかったりする」非決定性を生むため採用しない。
>   UI とマニュアルには高負荷になる旨を明記して利用者が選択時に判断できるようにする。

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
final pipeline では upscaler が `MAX_TEXTURE_DIM` 以下の target へ直接合成し、
UI スレッドの `clamp_for_gpu` に流さない。

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

アップスケールは target 合成でもタイル数・合成バッファ・推論時間が大きくなるため、既定値は従来相当の
`2048 x 2048 未満` のままにする。

ノイズ除去は 1x なので理屈上は `8192 x 8192` まで扱えるが、推論時間とメモリ負荷が大きい。
初期 UI ではアップスケールと同じ候補に揃え、8192 系は必要なら後で上級者向け候補として追加する。

## 4x 後の最終サイズ合成

現行アップスケールモデルはすべて 4x だが、将来 2x モデルを追加する可能性がある。
そのため「4x 決め打ち」ではなく、実際の出力サイズで GPU 上限を判定する。

要件:

- final AI 結果の `ColorImage` は `MAX_TEXTURE_DIM` 以下の target サイズへ直接合成する。
- 4x 全面 `ColorImage` を作ってから縮小する経路へ戻さない。
- `final_ai_cache` / `final_composite_cache` に極端な 16K 級画像を保持しない。
- target 合成後も `post_filter` / シャープ化 / GPU upload の順序は保つ。

実装:

- `run_final_ai_job` は `ai::upscale::upscale_to_max_dim(..., MAX_TEXTURE_DIM)` を呼ぶ。
- `ai::upscale` はモデル出力座標から target 座標へ逆写像し、既存の overlap weight を
  target 解像度で累積する。
- `FinalAiResult::Ready` / `final_ai_cache` / retained cache は `used_upscale` を保持する。

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

`ai::upscale` は、target サイズ全体に対して RGB 累積 3 面 + weight 1 面の `f32`
バッファを持ち、最後に `ColorImage` も作る。概算は target 1 pixel あたり最低 20 bytes
前後、アルファやタイル出力を含めるとさらに増える。下の表は旧「4x 全面を作ってから縮小」
方式なら発生していた中間サイズの目安で、現行方式では target は最大 8192x8192 に抑えられる。

例:

```text
1360 x 1920 を 4x        -> 5440 x 7680  = 41.8 MP  -> 約 0.8 GB 以上
2720 x 1920 を 4x        -> 10880 x 7680 = 83.6 MP  -> 約 1.7 GB 以上
2048 x 4096 を 4x        -> 8192 x 16384 = 134 MP   -> 約 2.7 GB 以上
4096 x 4096 を 4x        -> 16384 x 16384 = 268 MP  -> 約 5.3 GB 以上
```

`4096 x 4096 未満` 以上は、render-to-target 後も最大 8192x8192 の累積バッファと
多数のタイル推論を使うため、高スペック向けの上限として扱い、既定値にはしない。

### メモリガード方針 (2026-06-10 決定)

処理開始前の空きメモリチェックや `try_reserve` による graceful fail は**入れない**。
空きメモリという実行時状態で AI 適用可否が変わると、同じ画像・同じ設定でも結果が
変わり挙動を予測できなくなるため。適用可否はサイズ上限 (= ユーザーの明示選択) だけで
決定的に決まり、実メモリが不足する環境ではクラッシュ (allocator abort) を許容する。
代わりに UI とマニュアルへ高負荷になる旨を明記し、利用者が上限選択時に判断
できるようにする。

## 8192px 上限について

`MAX_TEXTURE_DIM = 8192` は wgpu / eframe の現在の初期化制約に合わせた上限。
実 GPU には 16384px 以上を扱えるものもあるが、アプリ側の wgpu limits、古い GPU /
ドライバ、描画コード、キャッシュ、分割テクスチャ対応まで巻き込むため、単純に上限を上げない。

5ch などで説明する場合は、
「互換性のため GPU テクスチャは 1 辺 8192px を上限にしています。
それ以上は分割描画が必要になり実装がかなり複雑になるため、当面は最終結果を 8192px 以下へ
直接合成する方向で対応します」
程度が安全。

## 実装箇所

- `src/settings.rs`
  - 新しいサイズ上限フィールドと default を追加。
  - 旧 `*_skip_px` からの移行を用意。
- `src/ui_dialogs/preferences/pages.rs`
  - AI 処理しきい値 UI を長辺 x 短辺の候補選択へ変更。
- `src/ai/upscale.rs`
  - `should_process_rect` を追加し、単体テストを追加。
  - final pipeline 向けに `upscale_to_max_dim` を追加し、target サイズへ直接合成する。
- `src/app.rs`
  - `should_process` 呼び出しをすべて新判定へ置き換える。
  - final AI 結果が 8192px 超のまま cache / texture upload へ流れないようにし、
    `used_upscale` を cache / retained entry に保持する。
- `src/ui_adjustment_panel.rs`
  - 「しきい値以上なので AI 無効」表示を新しい長辺 x 短辺表記へ変更。

## テスト観点

- 旧設定 `ai_upscale_skip_px = 2048` は新設定 `2048 x 2048 未満` と同じ挙動になる。
- `2720 x 1920` は `2048 x 2048 未満` では対象外、`4096 x 2048 未満` では対象になる。
- `1920 x 2720` のように縦横が逆でも同じ判定になる。
- アップスケール後の final AI 結果は、幅・高さとも `MAX_TEXTURE_DIM` 以下になる。
- AI 無効表示、先読み判定、パノラマ source 判定、final AI cache key 判定が同じ条件を使う。
- しきい値変更時に final AI cache / failed / pending が適切に無効化される。
- ~~4096 系を選んでも、処理開始前に明らかなメモリ過大ケースでクラッシュしない。~~
  → **取り下げ (2026-06-10 ユーザー判断)**: メモリ過大ガードは意図的に持たない。
  冒頭の実装サマリと「メモリ目安」の追記を参照。

## スコープ外

- GPU テクスチャ上限そのものを 8192px より上げる。
- 8192px 超画像の分割テクスチャ描画。
- ノイズ除去を、フルスクリーン表示用に 8192px 以下へ縮小される前の原寸画像へ掛ける処理。
