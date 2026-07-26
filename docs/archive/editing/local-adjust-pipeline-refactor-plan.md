# 補正レイヤー v1.1.0 — パイプライン再構成計画

**ステータス**: Codex 実装完了 / 自動検証 green / 実機 DoD 確認待ち (2026-06-03)
**スコープ**: v1.1.0 の主要アーキ変更
**工数見積**: 3-4 週間
**前提コミット**: HEAD (M-1〜M-7 + freeze fix + Phase 1 テスト infra 完了済)

---

## 0. なぜやるか — 1 行サマリ

> **「閲覧パイプライン」設計から「編集パイプライン」設計へ。AdjustParams 全項目 +
> AI 全種類 + post_filter を最終段へ移し、edit 系 (erase / conceal / local_adjust / crop)
> は常に source 解像度・補正前の状態で動作させる。**

得られるもの:
- マスク resize / アップスケール切替時のマスク消失バグが**構造的に消滅**
- 補正スライダー応答が **10-50 倍** 高速化 (edit 結果がキャッシュヒット)
- AI モデル (Real-ESRGAN / MI-GAN / 等) が**生 source を入力**として受け取り品質向上
- 編集 buffer が常に source 解像度 = ブラシ smoothness 改善
- cache 階層 5 段 → 2 段に simplify (= 今後のバグが構造的に減る)

---

## 1. アーキテクチャ Before / After

### 1.1 Before (現状: 閲覧パイプライン)

```
source                       (例: 1024×576)
  │
  ▼
[AdjustParams 適用]          ← 色調補正 (brightness 等)
  │
  ▼
[AI upscale (×4)]            (例: 4096×2304 になる)
  │
  ▼
[AI denoise]
  │
  ▼ ← この時点で edit 系の入力は「補正+upscale 後」
[erase 消しゴム]             ← この buffer サイズで動作
  │
  ▼
[conceal 隠蔽加工]           ← 同上
  │
  ▼
[local_adjust 補正レイヤー]  ← 同上
  │
  ▼
[crop]
  │
  ▼
[post_filter (レトロ等)]
  │
  ▼
[表示 / save]
```

**問題点**:
- AdjustParams のスライダーを動かすたびに、AI 以下すべての cache が無効化される
- AI モデルが「brightness +50 された画像」を入力にして学習分布から外れる
- マスクが upscale 後の解像度で焼かれる → upscale ON/OFF でマスクが消える
- edit buffer が常に大きい → ブラシ stroke 重い

### 1.2 After (新設計: 編集パイプライン)

```
source                       (例: 1024×576、ずっとこの解像度)
  │
  ▼
[erase 消しゴム]             ← source 解像度で動作 (= 軽い)
  │
  ▼
[conceal 隠蔽加工]           ← 同上
  │
  ▼
[local_adjust 補正レイヤー]  ← 同上
  │
  ▼
[crop]                       ← この段階で edit_result_cache (1 段目)
  │
  ▼ ─────────── ここから最終 composite stage (画像補正パネル) ───────────
  │
  ▼
[AdjustParams 適用]          ← 色調補正 (brightness 等)、cheap
  │
  ▼
[AI denoise]                 ← 生 source ベースの edit 結果に対して適用
  │
  ▼
[AI upscale (×4)]            (この段階で 4096×2304 に拡大)
  │
  ▼
[post_filter (レトロ等)]     ← 完全に最終出力エフェクト
  │
  ▼
[表示 / save]                ← final_composite_cache (2 段目)
```

**メリット**:
- edit 系のキャッシュは AdjustParams を含まない → スライダー操作で再計算不要
- AI 系が source の edit 結果 (純画像) を入力に受け取る = 品質向上
- マスク等の edit データは常に source 解像度 → upscale 切替で無効化されない
- edit buffer が小さい → ブラシ smoothness 改善

### 1.3 サムネイル

サムネイルには **最終段の画像補正一式 (AdjustParams + AI + post_filter) のみ反映** する。
edit 系 (erase / conceal / local_adjust / crop) はサムネには反映しない。

理由: サムネの目的は「フォルダ一覧での印象を補正で揃える」こと。edit 系は局所編集で、
サムネで見せても意味が薄い。また edit 系を反映するとサムネ生成が重くなる。

実装上の単純化:
- `thumb_adjust_tex` は AdjustParams のみで生成 (= 既存動作とほぼ同じ)
- 新パイプラインでは AdjustParams が "最終段" になっただけで、サムネ側の取り扱いは変わらない

---

## 2. 全 Phase 一覧 (Codex 実行順)

| Phase | 内容 | 工数 | 並行可能か |
|---|---|---|---|
| P1 | パイプライン再構成 (composite chain 全体) | 5-7 日 | × (基盤、最初) |
| P2 | cache 設計再構築 (5 cache → 2 cache) | 3-5 日 | × (P1 と同時) |
| P3 | display と edit の分離 (= 編集 source-res 化) | 2-3 日 | × (P1 P2 後) |
| P4 | conceal / local_adjust の source 解像度固定 | 2-3 日 | × (P3 後) |
| P5 | 既存データ互換性 + 動作確認 | 2-3 日 | △ (P3 P4 後) |
| P6 | Rayon 並列化 | 1-2 日 | ○ (P4 完了後) |
| P7 | Strategy C: brush stroke 時の effect deferral | 1-2 日 | ○ (P6 並行可) |
| P8 | エクスポート出力サイズ選択 | 0.5-1 日 | ○ (P1 完了後ならいつでも) |
| P9 | テスト + 退行確認 | 3-5 日 | × (最後) |
| P10 | ドキュメント + リリースノート | 1-2 日 | △ (P9 後) |
| **合計** | | **3-4 週間** | |

---

## 3. Phase 詳細

### P1. パイプライン再構成 (5-7 日)

#### P1.1 設計確定事項

- **AdjustParams 全項目** (brightness/contrast/gamma/saturation/temperature/black_point/
  white_point/midtone/auto_mode/upscale_model/denoise_model/post_filter) を最終段に移す
- **legacy pipeline トグルは作らない** (= 一括移行)
- edit 系 (erase/conceal/local_adjust/crop) はすべて **source 解像度** で動作
- 「Ctrl 押下中は元画像表示」「ctrl_shift 押下中は選択レイヤーまでプレビュー」等の
  既存表示モディファイアは挙動を維持

#### P1.2 実装手順

1. **新しい composite chain 関数を作成** (`src/app.rs` 付近に `fn compose_final_image(idx)`):
   ```rust
   pub(crate) fn compose_final_image(&self, idx: usize) -> Option<Arc<ColorImage>> {
       // 1. edit_result_cache から edit 結果取得 (= source 解像度の edit 完了画像)
       let edit_result = self.current_edit_result_pixels(idx)?;
       
       // 2. AdjustParams 取得 (effective_params 経由)
       let params = self.effective_params(idx);
       
       // 3. final_composite_cache に hit すれば return
       let key = FinalCompositeKey { idx, edit_gen, params_hash };
       if let Some(cached) = self.final_composite_cache.get(&key) {
           return Some(cached.clone());
       }
       
       // 4. 最終段 chain を順に適用:
       //    a. apply_adjust_params(edit_result, params) — 色調補正
       //    b. apply_ai_denoise (有効なら) 
       //    c. apply_ai_upscale (有効なら) — ここで初めて解像度が変わる
       //    d. apply_post_filter
       
       // 5. final_composite_cache に保存して return
   }
   ```

2. **edit_result_cache 新設** (= source 解像度の edit 完了状態):
   - キー: `EditResultKey { idx, edit_gen }` (= AdjustParams 含まず)
   - 値: `Arc<ColorImage>` (source 解像度)
   - 無効化トリガー: erase / conceal / local_adjust / crop の変更のみ

3. **既存の表示経路差し替え** (`src/ui_fullscreen.rs`):
   - `resolve_fs_processed_texture` 経路を `compose_final_image` 呼出に置換
   - `resolve_local_adjust_source_texture` は **edit_result_cache** から取る経路へ
     (= AI upscale 後ではなく source 解像度の erase 結果)
   - Ctrl 押下中の元画像表示は `current_local_adjust_source_pixels` (= 生 source) を返す

4. **既存 cache の段階的整理**:
   - `adjustment_cache` → **削除** (= final_composite_cache に統合)
   - `ai_upscale_cache` → final_composite_cache の途中段に統合 (詳細 P2 で)
   - `conceal_cache` → edit_result_cache 内部にネスト (詳細 P2 で)
   - `local_adjust_cache` → 同上
   - `erase_result_cache` → edit_result_cache 内部にネスト
   - **削除順序**: P2 で詳述 (一気に消すとビルド通らないので段階的に)

#### P1.3 注意点

- 既存の `apply_layers_with_progress` (core) のシグネチャは**維持**。core レベルでは
  「source → effect 適用 → 出力」の純関数性は不変。組み合わせ方が変わるだけ。
- `mask_dirty / generation` 概念は edit 側に閉じ込める。AdjustParams 側は独自に持つ。

---

### P2. cache 設計再構築 (3-5 日)

#### P2.1 新 cache 構造

```rust
// 1 段目: edit 結果 (source 解像度、AdjustParams 含まない)
pub(crate) edit_result_cache: HashMap<EditResultKey, EditResultEntry>,

struct EditResultKey {
    pub idx: usize,
    pub edit_gen: u64,  // erase/conceal/local_adjust/crop いずれかの変更で +1
}

struct EditResultEntry {
    pub pixels: Arc<ColorImage>,  // source 解像度
    pub texture: TextureHandle,
}

// 2 段目: 最終 composite (AdjustParams + AI + post_filter 適用後)
pub(crate) final_composite_cache: HashMap<FinalCompositeKey, FinalCompositeEntry>,

struct FinalCompositeKey {
    pub idx: usize,
    pub edit_gen: u64,        // 1 段目の世代
    pub params_hash: u64,     // AdjustParams のハッシュ
    pub ai_upscale_gen: u64,  // AI upscale 結果の世代 (cache busted on model change)
    pub ai_denoise_gen: u64,
}
```

#### P2.2 削除する既存 cache

- `adjustment_cache` (= final_composite_cache に統合)
- `local_adjust_cache` (= edit_result_cache に統合、edit 系のうちの 1 つとして扱う)
- `conceal_cache` (= edit_result_cache に統合)
- `erase_result_cache` (= edit_result_cache に統合)
- `ai_upscale_cache` は **残す** (= AI 結果を再利用するため、final_composite_cache とは別レイヤー)

#### P2.3 cache 無効化ルール (簡略化後)

| 変更 | 無効化対象 |
|---|---|
| erase 編集 | edit_result_cache(idx) + final_composite_cache(idx 全 hash) |
| conceal 編集 | 同上 |
| local_adjust 編集 | 同上 |
| crop 編集 | 同上 |
| AdjustParams 変更 (slider drag 等) | **final_composite_cache(idx, hash) のみ** ← edit 系は無傷! |
| AI モデル切替 | final_composite_cache(idx) + ai_upscale_cache(idx) |
| ページ移動 | keep_range 外を evict |
| フォルダ切替 | 全 cache クリア |

**スライダー操作の劇的高速化**はここ:
- 旧設計: スライダー動かすたびに edit 系 (= 重い) も再計算
- 新設計: edit_result_cache は無傷、final_composite_cache の AdjustParams 適用部分のみ再計算

#### P2.4 段階的削除手順 (ビルド通しながら)

1. 新 cache 構造 (edit_result_cache / final_composite_cache) を追加
2. `compose_final_image` を実装、既存 cache と並列に動かす (= 結果照合用)
3. 表示経路を 1 つずつ新 cache へ切替
4. 全経路切替完了したら旧 cache 削除
5. テスト全 pass

---

### P3. display と edit の分離 (2-3 日)

#### P3.1 目的

「画面に表示される pixels」と「edit 操作が target にする pixels」を別ソースにする。
- 表示: 最終 composite (AdjustParams + AI + post_filter 適用後)
- edit: source pixels (補正前、source 解像度)

ユーザー体感:
- 画面では brightness +50 された画像が見える
- ブラシをクリック → 座標は source pixel に変換 (= clicked position は同じ)
- マスクは source 解像度で保存
- マスクプレビュー overlay は source pixels を背景に合成 (= 編集中の見え方)

#### P3.2 実装

- `current_local_adjust_source_pixels(idx)` を **生 source** を返すように変更 (現状は erase 結果)
  - 別名で `current_edit_input_pixels(idx)` を追加し、edit 系の入力ソースとして使う
- マスクプレビュー overlay の background は edit_result_cache から取得
- 表示 texture は `compose_final_image` から取得 (= 最終 composite)

#### P3.3 座標変換

- edit 系は source 解像度なので、表示テクスチャ上の座標と source 座標を変換するヘルパーが必要
- 既存の `local_adjust_screen_to_norm` を流用可能 (正規化座標経由)
- ブラシ stamp 等は source 解像度で動作するので、edit-time の座標精度は AI upscale で
  決まる解像度に縛られない (= 1024×576 source なら sub-pixel 精度はそこまで)

---

### P4. conceal / local_adjust の source 解像度固定 (2-3 日)

#### P4.1 変更内容

- `ui_conceal.rs` の `conceal_mask_size` を **source 寸法** に変更
  - 現状: `[w, h]` は入力画像 (= AI upscale 後の寸法) で設定
  - 新: 常に source 解像度
- `local_adjust_image_dims(app, fs_idx)` を **source 解像度** を返すように変更
  - 現状: `current_local_adjust_source_pixels` の寸法を返す
  - 新: source pixels (= fs_cache の Static エントリ) の寸法を返す
- マスク resize utility (`resize_local_adjust_mask_bilinear`) は不要に
  - U²-Net 用途のみ残す (`run_local_adjust_u2netp_segmentation`)
  - 「アップスケール切替時のマスク resize」は構造的に不要になる

#### P4.2 既存マスクの migration

- 既存ユーザーの mask DB / sidecar には **upscale 後の解像度で焼かれたマスク**が残っている
- 起動時 / page load 時に検知して **source 解像度に resize**:
  ```rust
  if mask.width != source.width || mask.height != source.height {
      mask = resize_to_source(mask, source.width, source.height);
  }
  ```
- これは一度しか走らない (resize 後保存される)

---

### P5. 既存データ互換性 + 動作確認 (2-3 日)

#### P5.1 既存ファイル互換テスト

- `.miv` サイドカー: 既存形式そのまま読める
- `adjustment.db`: スキーマ変更なし
- `conceal.db`: スキーマ変更なし
- `local_adjust.db`: スキーマ変更なし
- マスク DB: スキーマ変更なし、ただし migration で寸法 resize

#### P5.2 結果が変わるパターンの予測 + 文書化

| 既存ユーザーが見るパターン | 旧結果 | 新結果 |
|---|---|---|
| 補正なし + AI upscale なし + edit のみ | 同じ | 同じ |
| 補正のみ (色調)、edit なし | ほぼ同じ | ほぼ同じ (commutative 近い) |
| 補正 + edit (erase/conceal/local_adjust) | edit が補正後画像で動作 | edit が source で動作 → **微差** |
| AI upscale + edit | edit が upscale 後で動作 | edit が source で動作、upscale は最後 → **明確に違う** |
| AI denoise + erase (inpaint) | inpaint が denoise 後画像で動作 | inpaint が source で動作 → **明確に違う、新結果のほうが品質高い** |
| post_filter + edit | edit が post_filter 適用前で動作 (一部前) | post_filter が完全最終段 → **見た目変わる** |

リリースノートで以下を明記:
- 「補正パイプラインを再構成しました。edit 系 (消しゴム / 隠蔽加工 / 補正レイヤー) は
   常に元画像に対して適用され、画像補正パネルの設定はすべて最終段で適用されるようになりました」
- 「AI モデル (アップスケール / デノイズ / インペイント) は生画像を入力として
   受け取るようになり、出力品質が向上しました」
- 「補正スライダー操作時の反映速度が劇的に改善されました」
- 「既存の編集データは自動で新パイプラインに移行されます。出力結果が微妙に変わる
   ことがありますが、必要に応じて再エクスポートしてください」

---

### P6. Rayon 並列化 (1-2 日)

#### P6.1 対象

- `crates/local-adjust-core/src/lib.rs` の全 `apply_*` 関数 (60+ 個、5258-9076 付近)
- 全 `eval_*_mask` 系関数 (4935-, 5125- 等)
- `apply_manual_override` 内のループ
- `apply_mask_opacity`
- `morph_alpha` / `box_blur_alpha`
- 新 composite chain の AdjustParams 適用 (`apply_brightness_contrast` 等、新規実装か既存流用)
- AI 系は既に async worker なので対象外

#### P6.2 パターン

```rust
// 旧
let mut out = Vec::with_capacity(src.len());
for px in src.chunks_exact(4) {
    let result = process(px, params);
    out.extend_from_slice(&result);
}

// 新
use rayon::prelude::*;
let out: Vec<u8> = src
    .par_chunks_exact(4)
    .flat_map(|px| {
        let result = process(px, params);
        result.into_iter().collect::<Vec<_>>()
    })
    .collect();
```

近傍参照 (blur 等) は per-row 並列化:
```rust
out.par_chunks_exact_mut(width * 4)
    .enumerate()
    .for_each(|(y, row)| {
        for (x_idx, px_out) in row.chunks_exact_mut(4).enumerate() {
            *px_out = compute_at(src, x_idx, y);
        }
    });
```

#### P6.3 テスト

`tests/local_adjust_core_parity.rs` に並列前後 equivalence test 追加:
```rust
#[test]
fn rayon_apply_tone_curve_matches_sequential() {
    let src = synthetic_image(4096, 2304);
    let params = ToneCurveParams::default();
    
    let seq = apply_tone_curve_sequential(&src, params);  // 旧実装を残しておく
    let par = apply_tone_curve(&src, params);
    
    assert_eq!(seq, par);
}
```

主要効果ごとに最低 1 本書く (= 全 60 効果検証は時間掛かりすぎなので、代表 5-10 個)。

---

### P7. Strategy C: brush stroke 時の effect deferral (1-2 日)

#### P7.1 目的

ブラシ stroke 中は **edit_result_cache の再計算をデバウンス**する。stroke 終了 (= 最後の入力から 150ms 経過) で初めて再計算 trigger。

ストローク中はマスクプレビュー overlay だけ更新される (= mask buffer 自体は更新済み、
但し edit_result_cache は前回値のまま) = 体感:
- マスクが描けている (overlay 即時更新)
- 効果適用結果は少し遅れて反映 (許容範囲)

#### P7.2 実装

```rust
pub(crate) fn paint_local_adjust_mask_brush_segment(...) {
    // ... mask buffer 更新 ...
    
    // 旧: bump_local_adjust_generation(idx) — 即時 cache invalidate + worker spawn
    // 新:
    self.bump_local_adjust_mask_preview_generation(idx);   // overlay だけ即時更新
    self.local_adjust_brush_stroke_last_input_at = Instant::now();
    self.local_adjust_brush_stroke_pending_idx = Some(idx);
}

// App::update の毎フレ:
fn update_local_adjust_brush_deferred_render(&mut self) {
    let Some(idx) = self.local_adjust_brush_stroke_pending_idx else { return; };
    if self.local_adjust_brush_stroke_last_input_at.elapsed() >= Duration::from_millis(150) {
        // 150ms idle → edit_result_cache invalidate + worker spawn
        self.bump_edit_generation(idx);
        self.local_adjust_brush_stroke_pending_idx = None;
    }
}
```

#### P7.3 リスク

- ユーザーが描き続けている間 effect 適用結果が古いまま = mask の正確性に対する違和感
- マスクプレビュー overlay の色が edit 結果を反映していない事に注意 (= 純粋にマスク表示のみ)
- 150ms の閾値は実機で微調整 (100-300ms の範囲で best feel を探す)

---

### P8. エクスポート出力サイズ選択 (0.5-1 日)

#### P8.1 仕様

Ctrl+E のエクスポートダイアログに「出力サイズ」ラジオボタンを追加:
- **そのまま** (= AI upscale 適用後の解像度): default、現状動作互換
- **1/2 サイズ** (= AI upscale 適用後の 50%)
- **1/4 サイズ** (= AI upscale 適用後の 25%、= 大体 source 解像度に近い)

ラジオボタンのラベルには実際の出力ピクセル数も表示:
```
出力サイズ: ⦿ そのまま (4096×2304)
            ○ 1/2 サイズ (2048×1152)
            ○ 1/4 サイズ (1024×576)
```

設定の永続化:
- `Settings::export_default_scale: ExportScale`
- 起動時 default、Ctrl+E 開いたとき初期選択
- ユーザーが変更したら次回も覚えている

#### P8.2 実装

```rust
// src/export_dialog.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ExportScale {
    #[default]
    Full,
    Half,
    Quarter,
}

impl ExportScale {
    pub fn factor(self) -> f32 {
        match self { Self::Full => 1.0, Self::Half => 0.5, Self::Quarter => 0.25 }
    }
    pub fn label(self) -> &'static str {
        match self { Self::Full => "そのまま", Self::Half => "1/2 サイズ", Self::Quarter => "1/4 サイズ" }
    }
}

pub struct ExportDialogState {
    // ... 既存フィールド ...
    pub scale: ExportScale,  // 追加
}
```

UI 追加 (~15 行、`draw_export_dialog` の format 行の下):
```rust
ui.horizontal(|ui| {
    ui.label("出力サイズ:");
    let dims = state.pixels.dims();
    for s in [ExportScale::Full, ExportScale::Half, ExportScale::Quarter] {
        let tw = (dims[0] as f32 * s.factor()).round() as u32;
        let th = (dims[1] as f32 * s.factor()).round() as u32;
        let label = format!("{} ({}×{})", s.label(), tw, th);
        ui.radio_value(&mut state.scale, s, label);
    }
});
```

Worker 側で適用 (~20 行、`spawn_export_worker` の save 直前):
```rust
let final_pixels = if request.scale != ExportScale::Full {
    let new_w = (orig_w as f32 * request.scale.factor()).round() as u32;
    let new_h = (orig_h as f32 * request.scale.factor()).round() as u32;
    crate::fast_resize::resize_rgba8_exact(
        &pixels, orig_w, orig_h, new_w, new_h,
        crate::fast_resize::Quality::Lanczos3,
    )?
} else {
    pixels
};
```

#### P8.3 spread (見開き) 対応

- spread エクスポート時も同じ scale を両ページに適用
- dialog の "(X×Y)" 表示は片ページ分 + 「各ページ」注記

#### P8.4 テスト

`tests/export_integration.rs` に追加:
- `fn export_with_scale_half_produces_half_dimensions()`
- `fn export_with_scale_quarter_produces_quarter_dimensions()`

---

### P9. テスト + 退行確認 (3-5 日)

#### P9.1 既存テスト全 pass 確認

- `cargo test` 全体 green
- ui_snapshot.rs の既存スナップショットが変わっていないか目視
- 既存 sidecar / DB ファイルでの動作確認 (= 開発者の手元に残っているテストデータで実機テスト)

#### P9.2 新規テスト

- `tests/local_adjust_core_parity.rs` に Rayon equivalence test (P6.3 で言及)
- `tests/export_integration.rs` に ExportScale test (P8.4)
- `tests/pipeline_integration.rs` (新規) — 新旧パイプラインで同じ入力で結果が
  「ほぼ同じ」(色差 < 閾値) になることを検証

#### P9.3 性能ベンチ

`tests/local_adjust_core_parity.rs` の `full_mask_with_large_subtract_buffer_completes_quickly`
パターンを参考に:
- `fn brightness_slider_drag_does_not_invalidate_edit_cache()` — スライダー操作後、
   edit_result_cache がヒットすることを確認 (= 設計の核)
- `fn brush_stroke_during_150ms_does_not_trigger_full_render()` — Strategy C の検証

#### P9.4 実機シナリオテスト (人手)

| シナリオ | 想定 |
|---|---|
| 4K 画像 + AI upscale ON + 補正レイヤー 5 つ + 明るさスライダー drag | 60fps 維持 |
| 同上 + ブラシ stroke | 60fps 維持 |
| 16K 画像 + 同上 | 60fps 維持 (上限は GPU upload 帯域) |
| AI upscale ON → OFF 切替 | マスクは消えない、結果は変わるが正しい |
| Ctrl+E で 1/2 サイズ export → 出力ファイル確認 | 期待通りの寸法 |

---

### P10. ドキュメント + リリースノート (1-2 日)

#### P10.1 設計ドキュメント更新

- `docs/display-pipeline.md`: 新パイプライン図に更新、AdjustParams が最終段に
- `docs/preset-and-adjustment.md`: cache 階層が 2 段になったことを反映
- `docs/archive/editing/local-adjust-pipeline-refactor-plan.md` (本書): "ステータス: 完了" に変更
- `docs/archive/editing/local-adjust-integration-audit.md` の関連項目を完了マーク
- `CLAUDE.md` の「修正対象の領域 → 読むドキュメント」表に変更点反映

#### P10.2 リリースノート (../../README.md / GitHub Release body)

```markdown
### v1.1.0

#### 補正レイヤー機能の追加

(...新機能の説明...)

#### 画像補正パイプラインの再構成 (重要)

「画像補正」パネル (明るさ・コントラスト・色温度・AI アップスケール・AI デノイズ・
ポストフィルタ) は **常に最終段** で適用されるようになりました。これにより:

- **補正スライダーの反映が劇的に高速化** されました (10 倍以上)
- 消しゴム / 隠蔽加工 / 補正レイヤーは元画像に対して動作するようになり、
  AI モデル (アップスケール・インペイント) の出力品質が向上しました
- アップスケール ON/OFF の切替でマスクが消える問題が解消しました

⚠️ **既存ユーザーの方へ**: 過去に消しゴムや AI アップスケールを使った画像を
再表示した場合、出力結果が以前のバージョンと微妙に異なることがあります
(新しい結果のほうが品質的に向上していますが、見た目の違いを感じる場合は
必要に応じて再エクスポートしてください)。

#### エクスポート出力サイズ

Ctrl+E のエクスポートダイアログで「そのまま」「1/2 サイズ」「1/4 サイズ」が
選べるようになりました。投稿サイズに合わせた出力が簡単になります。

#### 性能改善

- Rayon による効果適用の CPU 並列化 (約 6-8 倍速)
- ブラシ stroke 中の不要な再計算を抑制
```

---

## 4. リスク管理

### 4.1 高リスク

| リスク | 対策 |
|---|---|
| cache 設計バグ → 表示古いまま / 過剰再計算 | P9.2 で「edit cache が変わらないこと」のテストを追加 |
| 既存ユーザーの結果差異クレーム | リリースノートで丁寧に説明、AI 系は品質改善方向と明示 |
| 大規模 refactor のビルド通らない期間 | P2.4 段階的削除手順を厳守 |

### 4.2 中リスク

| リスク | 対策 |
|---|---|
| Rayon の equivalence バグ (浮動小数演算順序) | P6.3 で test、許容 epsilon 設定 |
| Strategy C の 150ms 閾値が体感に合わない | 実機で 100-300ms 試して best feel を選ぶ |
| ExportScale で Lanczos3 リサイズが遅い | fast_image_resize は AVX2 で十分速い (4K → 2K で <100ms) |

### 4.3 低リスク

| リスク | 対策 |
|---|---|
| Phase 8 が独立すぎて他 Phase と統合しづらい | P1 完了後ならいつでも追加可能、独立性高い |
| ドキュメントの更新漏れ | P10 でチェックリスト化 |

---

## 5. 完了基準 (Definition of Done)

以下すべてを満たせば v1.1.0 リリース可:

- [x] `cargo test` 全体 green、新規 test 追加分も pass
- [x] `cargo fmt --check` clean
- [ ] `cargo clippy` 退行 0
- [ ] 実機シナリオテスト (P9.4) 全 pass
- [ ] 既存 .miv / DB ファイルでの動作確認 OK
- [ ] スライダー drag 中の応答が現状比 10x 以上速い (実測)
- [ ] ブラシ stroke が 4K で 60fps 維持 (実測)
- [x] エクスポート時 1/2 サイズで出力ファイルサイズが想定通り (自動テスト)
- [x] ドキュメント更新 (../../display-pipeline.md / ../../preset-and-adjustment.md) 完了
- [x] リリースノート draft 完成

> Codex 確認済み: `cargo test` / `cargo fmt --package mimageviewer --check`。
> `cargo clippy --package mimageviewer --all-targets -- -D warnings` は
> `crates/local-adjust-core/src/lib.rs` 既存 lint 負債 (derivable_impls / too_many_arguments 等)
> で失敗。v1.1.0 release sign-off 前に別途 clippy debt closure が必要。
> 実機性能・既存ユーザーデータ確認はユーザー環境での sign-off 項目として残す。

---

## 6. Codex への進め方ガイドライン

1. **1 Phase 1 commit** で進める (= P1 = 1 commit、P2 = 1 commit、…)
2. 各 commit で `cargo test` 全 pass 確認、fmt clean
3. Phase 内でビルド通らない期間は許容 (= WIP commit OK)、ただし Phase 完了時には必ず通す
4. **commit message に Phase 番号を明記** (例: `[Pipeline P1] Move all adjustments to final stage`)
5. **わからない設計判断**は audit doc に質問追記して Claude Code に相談
6. **P5 P9 の実機シナリオ確認**は Claude Code がレビュー時に再現する
7. cache 設計など複雑な部分は実装前に Claude Code に「設計案」を投げてレビュー受けること推奨

---

## 7. Phase 着手順序の推奨

```
Week 1: P1 + P2 (基盤)
Week 2: P3 + P4 + P5 (edit 系の source-res 化)
Week 3: P6 + P7 + P8 (性能 + UX)
Week 4: P9 + P10 (検証 + ドキュメント)
```

並行可能な小タスク (P8) は隙間時間に進める。
