# トーン漫画の縮小モアレ対策 / 手動 LOD (mipmap 代替) 検討メモ

**ステータス: 段階実装。まず §4.4 の手動 post_filter 縮小フィルタを回避策として先行実装する
方針 (他セッションの並行作業が落ち着いてから着手予定、2026-07-08 合意)。§4.2 / §4.3 の LOD に
よる根本的解決は将来再検討する。**
これは調査結果と対策方針をまとめた設計メモ。
バックログ本体は `docs/next-release-backlog.md` §3.4 (要約 + 本ファイルへのリンク)。

先行実装 (§4.4) は post_filter 内に閉じるので detached-rework 凍結ルールの影響は小さい。
LOD による根本的解決 (§4.2 / §4.3) に着手する場合は、まず **§8 着手前の前提**
(特に detached-rework 凍結ルール) を読むこと。

---

## 1. 背景・症状

- ユーザー報告 (2026-07-07): トーン (スクリーントーン = 規則的な網点) を貼った漫画を
  縮小表示すると **モアレ** が出るのが気になる。
- 原理: トーンは高周波の規則パターン。縮小すると、縮小後のナイキスト周波数を超えた
  成分が折り返して低周波の縞になる (aliasing / moiré)。適切な prefilter (low-pass) が
  効かない縮小をするほど顕著に出る。

## 2. 現状の縮小パイプライン (調査結果)

### 2.1 縮小の実体

縮小は全て `src/fast_resize.rs` の `fast_image_resize` (AVX2 / SSE4.1 SIMD) ラッパに
集約されている。フィルタは **Bilinear (≈ Triangle)** と **Lanczos3** の 2 択のみ
(`Quality` enum)。新規リサイズは `image::DynamicImage::resize` を直接呼ばずこの経路を使う規約。

### 2.2 サムネイル一覧 (`src/thumb_loader.rs`)

- JPEG は TurboJPEG の **DCT スケール (1/8・1/4・1/2・1/1 の離散段)** で縮小デコード →
  残りを **Lanczos3** で `display_px` へ (`resize_to_display_color_image`)。
- `display_px` = セルサイズ × DPI を 256〜2048 にクランプ (`compute_display_px`)。
  **ほぼ実表示サイズと一致**するので、GPU 側の追加縮小はごくわずか。
- GPU 表示は `TextureOptions::LINEAR` (bilinear, mipmap なし)。

### 2.3 フルスクリーン表示 (`src/ui_fullscreen.rs`) ← モアレ主因

- `fs_cache` は **原寸** (`clamp_dynamic_for_gpu` で最大 8192px にクランプするだけ、
  クランプは速度優先の Bilinear) を GPU テクスチャとして保持する。
- **表示サイズへの CPU 縮小はしない**。`draw_fs_image` が原寸テクスチャを `fit_scale` で
  縮小表示する (`tex_size = handle.size_vec2()` → `fit_scale` → `img_rect`)。
- GPU が `TextureOptions::LINEAR` (bilinear, **mipmap なし**) で minification サンプリング。
  縮小率が 0.5 を切ると入力画素を飛ばしてサンプリングするため、トーンの高周波が
  折り返して**強いモアレ**になる。例: B5/300dpi スキャン 3508px を 1440px モニタに
  縦フィット = 2.4 倍縮小。

### 2.4 縮小・最終合成がどのスレッドで走るか

- `clamp_dynamic_for_gpu` (`src/app.rs`) は **worker スレッド向け**と明記。
  `start_fs_load` の `std::thread::spawn` 内で呼ばれる。**= 原寸→8192 の縮小は
  既に worker で日常的に走っており UI をブロックしていない**。
- 一方、最終合成 `build_final_composite_texture_from_base` (`src/app.rs`) は
  **UI スレッドで同期実行**され (`ensure_final_composite_texture` のコメントに明記)、
  大判画像では `post_filter` + GPU upload が **数百 ms かかりうる**。

## 3. モアレの原因整理

1. **フルスクリーン (主因)**: 原寸を GPU の naive bilinear で大縮小、mipmap なし。
2. **サムネ (副次)**: Lanczos3 は sinc 系で**負のローブ**を持ち、高コントラストの規則
   パターン (トーン) でリンギング → モアレを強調しやすい。ただし convolution は縮小時に
   カーネル幅を広げる prefilter を内包するので、GPU bilinear ほど酷くはない。
3. **post_filter の適用解像度**: 現状は原寸で適用 (§6 参照)。疑似カラー・減色など
   規則パターンを足す系のフィルタは、原寸で掛けてから縮小すると**フィルタが作った
   パターン自体が新たなモアレ源**になり得る。

## 4. 対策候補の評価

### 4.1 native mipmap (egui/eframe の `TextureOptions.mipmap_mode`) → **不可**

依存クレート実ソース (0.33.3) を確認した結果:

- `epaint/src/textures.rs:170` のドキュメントに **「currently only `egui_glow`」** と明記。
  wgpu バックエンドの mIV では効かない。
- `egui-wgpu/src/renderer.rs:684`: ユーザーテクスチャは `mip_level_count: 1` 固定。
- `egui-wgpu/src/renderer.rs:1053 create_sampler`: `magnification` / `minification` /
  `wrap_mode` だけを見て **`mipmap_mode` を完全に無視**。`SamplerDescriptor` に
  `mipmap_filter` も渡していない。
- → `TextureOptions::LINEAR.with_mipmap_mode(Some(Linear))` にしても mIV では**何も起きない**。

### 4.2 CPU 高品質縮小してからアップロード (最小実装)

- 既存 `fast_resize` (Lanczos) を流用、egui 標準 `ctx.load_texture` 経路のまま。
- 8192 → 表示解像度の縮小は **worker でやれば UI ブロックなし** (`clamp_dynamic_for_gpu`
  が既に worker で 8192 縮小をやっている実例。むしろ `8192→2560` は `8192clamp` より
  書き込み画素が少なく軽い)。
- フィット表示は倍率固定なので **1 枚縮小で済む**。ズームで拡大する時はモアレが出ないので、
  **フィット時=縮小版 / ズーム拡大時=原寸** の二枚持ちにすれば、ズーム操作の滑らかさは
  現状と同一のまま、フィット表示のモアレだけ消える。

### 4.3 手動 LOD ピラミッド (手動 mipmap)

4.2 を N 段に一般化。原寸 + 1/2 + 1/4 + … を用意し、表示倍率に対して **「1 つ大きい
レベル」を選んで貼る**。GPU の実効縮小率が常に [0.5, 1.0] に収まるので bilinear でも
モアレがほぼ出ない (= mipmap の原理そのもの)。

- 品質は Lanczos で作れば GPU 自動 mipmap の box フィルタより上。トーン向き。
- ズームで倍率が変わってもレベル切替だけ = 連続再縮小なし = 滑らか。
- 生成方法:
  - **CPU** (worker で段階 Lanczos): 実装は中。連結読みで大量ページだと CPU 負荷に注意
    (ただし worker なので UI には出ない)。
  - **GPU** (wgpu の render pass / compute で pyramid 生成 + `egui-wgpu` の
    `register_native_texture_with_sampler_options` で native texture 登録): 操作の
    滑らかさには本命だが実装は大。既存 `panorama_wgpu.rs` / `compare_wgpu.rs` の
    wgpu アクセスを流用できるが、テクスチャのライフサイクル管理 (egui TextureId 整合・
    フレーム跨ぎ保持・drop) と編集更新時の再生成が絡む。
- **重要 (Codex 指摘・確認済み)**: `draw_fs_image` は `handle.size_vec2()` を画像の
  論理サイズとして使っており、この用法は `ui_fullscreen.rs` 内に **10 箇所以上**ある
  (単ページ / 見開き left・right 2 枚 / ルーペ / detached passive window など)。
  縮小段のテクスチャをそのまま渡すと「画像そのものが 1/4 サイズ」と誤解し、原寸表示・
  ズーム・ルーペ・pixel grid がずれる。実装時は次の分離を全経路で貫く:
  - **レイアウト計算は元の画像サイズ**で行う
  - **描画に使う `TextureHandle` だけ**縮小段へ差し替える
  - **UV は 0..1 のまま**使う
  - **ルーペは原寸固定** (拡大鏡なので LOD だとボケる)、**pixel grid は論理サイズ必須**
    なので LOD 選択から除外する特別扱いが要る。

### 4.4 手動 post_filter 縮小フィルタ (先行実装する回避策) ★当面はこれで対応

4.2 / 4.3 の LOD による根本的解決は範囲が大きいため、当面はユーザーが手動で選ぶ
post_filter として **1/2 / 1/4 縮小**を用意し、モアレを自衛できるようにする。

**なぜ軽いか**: `post_filter` は既に **CRT 系 (`CrtSimple` 等) が出力サイズを 2〜4 倍に変える**
道を通しており (`crt_upscale_factor` / `CRT_OUTPUT_MAX=4096`)、下流 (`build_final_composite`
→ `draw_fs_image` の `size_vec2()` ベースのレイアウト) が **post_filter 後のサイズを吸収**する。
したがって 4.3 で挙げた「別テクスチャで裏持ちすると `size_vec2()` でずれる」問題は起きず、
下流のレイアウト改修は基本不要。フィット表示では `fit_scale` がサイズ差を吸収するので、
**画面上の表示サイズは変わらず GPU 実効縮小率だけ緩和** = モアレが減る。

**確認済みの下流の整合**:
- pixel grid は `zoom > ほぼ等倍` のときだけ描画 (`should_draw_fs_pixel_grid`)。縮小フィルタが
  効くフィット表示では出ないので衝突しない。
- 原寸表示のホバー情報 / 「⚠ ダウンスケール表示中」警告は `fs_cache` 側 (原寸) から読むので
  正しく動く (縮小フィルタ選択中に警告が出るのはむしろ自然)。
- 切替キーは既存の T = `KeyAction::FsPostFilterNext` が `PostFilter::ALL` を巡回するので、
  ALL に足すだけで T 巡回・操作カスタマイズに自動で乗る (新規アクション不要)。

**段数**: 1/2・1/4 の 2 択。大判スキャン (6000〜9000px) は 1/4 (例 2048→1440 = 1.4x) でほぼ消え、
中判 (2500〜3500px) は 1/2。原寸が表示解像度より小さい画像に強い縮小をかけると拡大ボケする
(手動選択なので選び直せる)。

**触るファイル**:
- `src/adjustment.rs`: `PostFilter` に variant 2 つ + `label()` 2 行 + `PostFilter::ALL` に 2 つ
  (serde は `#[serde(rename_all = "snake_case")]` の文字列 enum なので後方互換・移行不要)。
- `src/post_filter.rs`: `apply` に 2 分岐 + 縮小関数 (`fast_resize` 流用、ColorImage↔RgbaImage 変換込み)。
- `src/ui_adjustment_panel.rs`: ComboBox に `selectable_value` 2 行。
- `src/app/gamepad_input.rs`: post_filter ドリルグループ分類に 2 つ (テスト
  `post_filter_drill_groups_cover_all_filters_once` が全フィルタ 1 回網羅を検証、入れないと落ちる)。
- ドキュメント: `htdocs/.../manual/` 補正説明、`docs/preset-and-adjustment.md`。

**制約 (仕様として明示)**:
- **フルスクリーン専用**。サムネ一覧には post_filter が適用されない (display-pipeline.md §1.4) ため
  一覧のモアレには効かない。
- **他の post_filter と排他** (1 度に 1 つ)。縮小 + 疑似カラー等の併用は不可。
- **静的倍率** (表示解像度・画像サイズで最適段が変わるので手動で T 巡回して選ぶ)。
- 見た目変更なので **実機目視確認が必須** (CLAUDE.md「実機検証用バイナリの準備」)。

**位置づけ**: あくまで回避策。表示解像度に自動追従せず、解像感も 4.3 の LOD より劣る。
気になるユーザーが自衛する手段として先行提供し、根本的解決 (4.2 / 4.3) は将来別途検討する。

## 5. 連結表示・見開きとの相性

- 連結読み (continuous) は 1 枚の巨大テクスチャではなく、**ページ単位の個別テクスチャを
  rect 計算で並べる方式** (`ContinuousReadingUnitSize { pages: Vec<ContinuousReadingPageSize> }`、
  `continuous_reading_page_rects` in `ui_fullscreen.rs`)。
- 各ページが個別に `fs_cache` / `final_composite` を引くので、「ページ単位で表示解像度に
  縮小 / LOD 選択」という案がそのまま乗る。連結は横幅フィットで各ページが小さく表示され
  **縮小率が大きい = 今最もモアレが出やすい**ので、恩恵が大きい。
- 見開き (2 枚並べ) も同じくページ単位テクスチャなので同様。

## 6. post_filter / 編集との順序

- `final_composite` の実体 `build_final_composite_texture_from_base` の順序は
  `edit_result_cache(原寸) → smart_sharpen → post_filter → clamp_for_gpu → load_texture`。
  **post_filter がかかる時点でテクスチャは原寸** (表示縮小されていない)。
- 「最後の方に縮小を差す」場所としては post_filter 後・`load_texture` 前 (`clamp_for_gpu`
  の位置) が構造上の候補だが、**そこは UI スレッド**なので単純に足すと UI が引っかかる。
- モアレ対策としての理想順序:
  ```
  原画を表示解像度へ Lanczos 縮小 (= 原画トーンの高周波を落とす)
    → その上で post_filter を表示解像度で掛ける
  ```
  つまり縮小は **post_filter の "前"**、かつ表示解像度基準でフィルタを掛けるのが筋が良い
  (`needs_nearest_sampler` の NEAREST 系フィルタは特に表示解像度で決めるべき)。
- 編集 (消しゴム / 補正レイヤー / AI) は **原寸で行う**必要がある (精度)。LOD は
  「最終表示ソースからの派生」と位置づけ、補正 / AI / 注釈で最終表示ソースが変わるたびに
  pyramid を作り直す。= 無効化を編集の全経路に配線する必要がある。

## 7. 実装コスト見積もり

| # | 作業 | 内容 | コスト |
| --- | --- | --- | --- |
| A | LOD 生成 | CPU: worker で Lanczos 段階縮小 (既存 `fast_resize` 流用)。GPU: wgpu pyramid + native texture 登録 | CPU:中 / GPU:大 |
| B | キャッシュ層の LOD 化 | `fs_cache` / `final_composite_cache` / `edit_result_cache` を単一 `TextureHandle` → LOD セットへ拡張。無効化ロジック (`clear_adjustment_caches` 等) も全レベル落とす | 2段:小 / N段:中 |
| C | 描画の分離 | `size_vec2()` を使う 10+ 経路で「論理基準サイズ」と「貼る handle」を分離。ルーペ原寸固定・pixel grid 論理サイズ必須の除外 | 中 |
| D | レベル選択 | `total_scale` から最適 LOD を選ぶ純関数 | 小 |
| E | メモリ整合 | `keep_range` / prefetch / evict で LOD セット全体を持ち回り・drop | 小〜中 |
| F | detached 凍結ルール | 表示テクスチャ経路は detached とも共有。着手前に `docs/detached-rework-plan.md` §2 を読み、viewer 述語・viewport 経路に触れない範囲へ最小化 | 制約 (要注意) |
| G | テスト・実機検証 | `final_composite_cache` 系の回帰テスト (tests.rs に数十件、単一 texture 前提) を更新。Windows ネイティブ表示なので実機目視も必須 | 中 |

**mIV 固有の押し上げ要因**: C の分離が広い / B の無効化を編集全経路へ配線 / G のテスト更新量が
読めない / F の detached 凍結ルール (今リワーク中で根幹の表示テクスチャ構造に触るのは相性が悪い)。

## 8. 推奨する段階投資

0. **(先行・当面の対応) 手動 post_filter 縮小フィルタ (§4.4)**。ユーザーが T 巡回で 1/2 / 1/4 を
   選ぶ手動回避策。小規模で下流改修がほぼ不要、detached-rework 凍結ルールの影響も小さい。
   まずこれを入れて気になるユーザーが自衛できるようにする。
1. **CPU 2 段 (原寸 + 表示解像度)** から。フィット / 連結 / 見開きのモアレの大半がこれで消える。
   C の分離を `resolve_fs_processed_texture` に集約し、ルーペ / pixel grid を原寸固定にすれば
   閉じ込められる。**中コスト**。
2. 足りなければ **CPU N 段 LOD** に段数を増やす。
3. ズームで拡大縮小を頻繁に往復するワークフローの滑らかさまで求める場合のみ **GPU pyramid**。
   コスト大 + 凍結ルールのリスク。

### 着手前の前提

- **detached-rework 凍結ルール** (`CLAUDE.md` / `docs/detached-rework-plan.md`) の下にある。
  表示テクスチャ経路は detached と共有するので、まず detached-rework プラン §2 で
  「触ってよい表示テクスチャ経路の境界」を確定させるのが最初の一手。
- 表示テクスチャ優先順位 (`display-pipeline.md` §2.3) と合成順序 (§2.4) は動かさない。
  縮小は `load_texture` 前段 (worker 側の生成) に閉じ込め、優先順位・合成順序に影響させない。

## 9. 参照コード / ドキュメント

- `src/fast_resize.rs` — `Quality` (Bilinear/Lanczos3)、`resize_dynamic_fit/exact`、`probe_dims`
- `src/thumb_loader.rs` — `resize_to_display_color_image`、`compute_display_px`、DCT スケール
- `src/app.rs` — `clamp_dynamic_for_gpu` (worker)、`start_fs_load`、
  `build_final_composite_texture_from_base` / `ensure_final_composite_texture` (UI スレッド)
- `src/ui_fullscreen.rs` — `draw_fs_image`、`resolve_fs_processed_texture`、
  `continuous_reading_*`、ルーペ / pixel grid
- `src/post_filter.rs` — 疑似カラー / 減色などのポストフィルタ
- `docs/display-pipeline.md` — §1 サムネ / §2 フルスクリーン / §2.3 テクスチャ優先順位 / §2.4 合成順序
- `docs/preset-and-adjustment.md` — 補正 / AI / post_filter の適用順
- 依存クレート実ソース (mipmap 非対応の根拠): `epaint-0.33.3/src/textures.rs`、
  `egui-wgpu-0.33.3/src/renderer.rs`
