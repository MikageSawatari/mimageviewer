# 消しゴム/隠蔽キャッシュ層リファクタ計画 (Step 2)

mimageviewer の消しゴム (erase) + 隠蔽加工 (conceal) + 画像補正 (adjustment) + AI
アップスケール周りの **cache 層が散らかっていて invalidate 漏れ bug が頻発する**
構造問題を解決するためのリファクタ計画。

## なぜ必要か (背景)

過去 5 ラウンドの実機 FB / Codex review で繰り返し以下のパターンの bug を踏んだ:

| ラウンド | 症状 | 直接原因 |
| --- | --- | --- |
| R3 P1 | AI cache 復元で AI upscale が消える | `mask_delete_clicked` で復元画像を ai_upscale_cache に入れた直後 `invalidate_derived_fs_caches` で消える |
| R4 P1 | preview MI-GAN 結果が fs_cache に焼き込まれ ESC 後復活 | preview が fs_cache を上書きしていた → R4 で `erase_preview_cache` 隔離 |
| R4 #3 | AI OFF にしても upscale が残る | 消しゴム commit が fs_cache を「加工済み画像」で上書きしていて、AI cache を捨てても fs_cache が raw に戻らない |
| R5 P2 | preview→shape 移動/削除→再 preview で古い結果が一瞬見える | Delete / handle drag / Body drag の 3 経路で `clear_erase_preview` 忘れ |
| R5 P1 | preview 解像度ミスマッチで egui-wgpu assert panic | AI upscale が後から完了して、erase_mask_size (1x) と ai_upscale_cache (4x) が食い違うとき、preview コードが mismatched source を MI-GAN に投入 |

**根本原因**: `fs_cache / ai_upscale_cache / adjustment_cache / erase_base_cache /
erase_preview_cache / erase_base_tex_cache / conceal_cache` が個別に invalidate
される設計で、

- `fs_cache` が **raw decode** と **消しゴム commit 後の加工済み画像** の両方を
  担っている (= 責務混在、AI OFF で raw に戻れない原因)
- 新しい mutation 経路を追加するたびに `clear_*` を全部呼ぶ必要がある
  (= 漏れる)
- key に generation が無いので、手動 invalidate を忘れると stale entry を採用して
  しまう

## ゴール

1. **`fs_cache` = raw decode 専用** に確定する。消しゴム / 隠蔽 / 補正 / AI 結果は
   絶対に `fs_cache` を上書きしない。
2. **`erase_result_cache` を新設** して MI-GAN inpaint 結果を保持する。
3. **表示パイプラインを enum or 順序固定関数で型表現**: `Raw → AI → Adjustment →
   EraseResult → Conceal` の順序を**コードに表現する**ことで、新 layer 追加時の
   実装漏れを compile-time で検出可能にする。
4. **mutate_erase_mask / mutate_conceal_mask 中央集約関数**: mask / shapes 変更
   経路を全部この関数に通し、関数内で自動的に generation bump + 関連 cache の
   invalidate を行う。手動で `clear_*` を呼ぶ箇所を**排除**する。
5. **cache key に generation を入れる**: `erase_result_cache[(idx, input_gen,
   mask_gen)]` のようにして、手動 invalidate に依存しない設計にする。

このゴールが達成されると、現状の以下の問題が自動的に解決する:

- AI OFF にしてアップスケールが残る (= R4 #3)
- preview / commit pending が同 idx で潰し合う (= R5 P2 後段)
- 補正変更時にどの cache を invalidate するか手動依存
- 上記表に出ていない、未発見の同種 bug

## 触る範囲 / 触らない範囲

### 触る範囲

- `src/app.rs` (cache フィールド定義 + 表示テクスチャ解決メソッド)
- `src/ui_erase.rs` (mask 変更経路 + MI-GAN 結果受け取り)
- `src/ui_conceal.rs` (mask 変更経路 + 合成テクスチャ生成)
- `src/ui_fullscreen.rs` (表示パイプライン)
- `src/ui_adjustment_panel.rs` (slider commit → adjustment_cache 連鎖)

### 触らない範囲 (= 互換性を維持)

- **`src/save_with_metadata.rs`** (= Phase 5 で完成済み、JPEG/PNG/WebP 保存)
- **`src/vector_edit.rs`** (= 既に純粋関数で hit_test / apply_drag を提供している、
  cache とは独立)
- **`src/mask_db.rs` / `src/conceal_db.rs` / `src/adjustment_db.rs`** SQLite スキーマ
  (= 既存ユーザーデータと互換、リリース済み)
- **`src/ai/runtime.rs` / `src/ai/upscale.rs` / `src/ai/inpaint`**
  (= AI 推論本体は変えない、ORT セッション + worker thread + cancel pattern は維持)
- **動画 / フォルダ走査 / settings 永続化** (= 補正パイプラインと無関係)

## 目標アーキテクチャ

### Cache layer の責務 (リファクタ後)

```rust
// src/app.rs (App 構造体)

/// レベル 0: 生デコード画像 (= ファイルから読んだそのまま、不変)。
/// 消しゴム / 隠蔽 / 補正 / AI 結果はここに**書き戻さない**。
pub(crate) fs_cache: HashMap<usize, FsCacheEntry>,

/// レベル 1: AI upscale 結果 (4x dim)。fs_cache → AI 入力 → アップスケール完了で生成。
pub(crate) ai_upscale_cache: HashMap<(usize, u8 /* bg */), FsCacheEntry>,

/// レベル 2: 補正適用後 (= brightness/contrast/gamma/...)。AI 完了 or fs_cache 変化で
/// 入力 generation が動き、stale 化する。
pub(crate) adjustment_cache: HashMap<usize, FsCacheEntry>,

/// レベル 3: 消しゴム inpaint 結果 (新設)。MI-GAN 結果を保持。
/// 旧設計は fs_cache を上書きしていたため、AI OFF で raw に戻れなかった。
pub(crate) erase_result_cache: HashMap<EraseResultKey, FsCacheEntry>,

/// レベル 3.5: preview 専用 (= ESC で破棄、DB / fs_cache を汚さない)。
pub(crate) erase_preview_cache: HashMap<usize, ErasePreviewCacheEntry>,

/// レベル 4: 隠蔽合成結果 (= mosaic/fill/blur)。erase_result または adjustment の
/// 上に重ねる。
pub(crate) conceal_cache: HashMap<usize, ConcealCacheEntry>,

/// 生成カウンタ (cache key に焼き込む)。
pub(crate) input_generation: HashMap<usize, u64>,  // ai / adjustment 変化で +1
pub(crate) erase_mask_generation: HashMap<usize, u64>,  // mask / shapes 変化で +1
pub(crate) conceal_mask_generation: HashMap<usize, u64>,
```

### Generation 入りキー

```rust
#[derive(Hash, Eq, PartialEq, Clone)]
pub(crate) struct EraseResultKey {
    pub(crate) idx: usize,
    pub(crate) input_gen: u64,  // ai_upscale または adjustment の generation
    pub(crate) mask_gen: u64,   // erase_mask_generation
}
```

これにより、入力 (AI / 補正) が変わっても、mask が変わっても、自動的に違うキーに
なるので **古い entry を誤って表示することがない**。HashMap には複数 entry が
残るが、`ensure_erase_result` の hit_check で見つからなければ再計算するだけ。
LRU eviction で適宜掃除 (= 既存 cache_eviction policy を流用)。

### 表示パイプラインの型表現

```rust
// src/ui_fullscreen.rs

/// 表示パイプラインの 1 層。実テクスチャ参照 or 次層へのフォールバック。
enum DisplayLayer {
    Raw,             // fs_cache
    AiUpscale,       // ai_upscale_cache
    Adjustment,      // adjustment_cache
    EraseResult,     // erase_result_cache (新設)
    EraseLivePreview, // erase_preview_cache (= 押下中のみ最優先)
    ConcealComposite, // conceal_cache
}

/// 表示用テクスチャを top-down で resolve する。
/// 各 layer は「自分の cache が hit すればそれを返す、miss なら下層へ」。
fn resolve_display_texture(&mut self, ctx, idx) -> Option<TextureHandle> {
    // 上層から順に try。最初に hit したものを採用。
    self.try_erase_live_preview(idx)      // 押下中のみ
        .or_else(|| self.try_conceal_composite(ctx, idx))
        .or_else(|| self.try_erase_result(ctx, idx))
        .or_else(|| self.try_adjustment(ctx, idx))
        .or_else(|| self.try_ai_upscale(ctx, idx))
        .or_else(|| self.try_raw_fs_cache(ctx, idx))
}
```

これで「どの layer を見て、どの順に重ねるか」が**コードに直接出る**。
新 layer 追加時はこの enum / chain に追加するだけ。

### Mutate 中央集約

```rust
// src/ui_erase.rs

/// 消しゴム mask / shapes 変更の唯一の窓口。
/// 直接 `self.erase_mask = ...` や `self.erase_shapes.push(...)` を**書かない**こと。
/// すべての mutation はこの関数経由にする。
pub(crate) fn mutate_erase_mask(
    &mut self,
    idx: usize,
    f: impl FnOnce(&mut EraseMaskState),
) {
    let state = EraseMaskState {
        mask: &mut self.erase_mask,
        shapes: &mut self.erase_shapes,
        selected_shape: &mut self.erase_selected_shape,
    };
    f(state);
    // 自動でやる:
    *self.erase_mask_generation.entry(idx).or_default() += 1;
    self.erase_mask_texture = None;          // overlay 再生成
    self.erase_preview_cache.remove(&idx);   // preview 破棄
    // erase_result_cache は key に mask_gen が入っているので自動で stale 化
}
```

これで、過去 5 ラウンドで踏んだ「Delete で preview 残る」「drag で preview 残る」
スタイルの bug が**構造的に発生不可能**になる。

`mutate_conceal_mask` も同様に作る。

## 既知の落とし穴 (リファクタ時に再導入しないこと)

1. **premultiplied alpha**: `egui::Color32` は premultiplied。RGBA バッファとして
   読み書きするときは `to_srgba_unmultiplied()` を必ず通す
   (= `src/save_with_metadata.rs` の前例参照)。

2. **Visuals::dark()**: フルスクリーンパネルは Light テーマでも黒背景統一。
   既存パネルは `*ui.visuals_mut() = egui::Visuals::dark();` +
   `ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);` を入れている。

3. **Frame::popup + ScrollArea**: ヘッダ (タイトル + プレビュー + ×) は ScrollArea
   の**外**に出す。中に入れるとスクロールバーが × ボタンに重なる。

4. **解像度ミスマッチ**: AI upscale は 4x dim を生成。`erase_mask_size` (= raw 寸法
   時に enter_erase_mode で固定) と AI cache (= 4x) のサイズ食い違いに注意。
   今回のリファクタでは `EraseResultKey.input_gen` が AI 完了で動くので、結果 cache
   は自然に新キーになる。**入力 pixels と composite mask は寸法一致を assert する**。

5. **preview / commit pending の衝突**: `EraseInpaintPending` を `is_preview: bool`
   で識別している。リファクタ後は **`pending: HashMap<(idx, kind), Pending>`** にして、
   preview と commit を独立 cancel 可能にすると良い (= Codex R5 P2 後段の指摘)。

6. **IME 入力**: TextEdit を含むダイアログで Enter/Escape は
   `self.dialog_enter_pressed(ctx)` / `dialog_escape_pressed(ctx)` を経由する。
   このリファクタでは TextEdit を新規追加しない想定だが、もし必要なら
   `src/app.rs` の helper を必ず使う (= 直接 `ctx.input(|i| i.key_pressed(...))`
   を呼ばない)。

7. **post_filter bypass**: `erase_mode = true` の間は `post_filter_bypassed = true`
   で post_filter を飛ばす。リファクタ後も `Adjustment` layer は post_filter 含め
   ない (= 既存と同じ動作)。

8. **見開き左右ピボット**: `enter_erase_mode` は見開きから入ると左ページへピボット
   する。`erase_spread_ctx` で旧 spread_mode を保存。リファクタでは触らない。

9. **Susie / 動画 / PDF / ZIP**: cache 経路は今回のリファクタ対象に絡まない。
   ただし `fs_cache_source_dims` のような小ヘルパーが既存箇所で参照されているので、
   削除前に grep で利用点を確認すること。

## 検証手順

### 自動テスト

```bash
cargo test --lib              # 1383 件 → 数件追加で 1390+ 想定
cargo test --test ui_snapshot # 13 件
cargo fmt --check             # clean を維持
cargo build --release         # warning 0
```

新規ユニットテストの追加 (TDD 推奨):

- `mutate_erase_mask` を呼ぶと `erase_mask_generation` が +1 され、`erase_preview_cache`
  から該当 idx が消えること
- `EraseResultKey` の `input_gen` が変わると古い entry を `ensure_erase_result` が
  使わないこと
- `fs_cache` は消しゴム commit 後も raw decode のままであること (= AI OFF にして
  fs_cache を直接見ると AI 適用前の画像が見えること)
- preview / commit が同じ idx で同時実行されても、commit pending を cancel しない
  こと

### 手動回帰テスト

実機で以下を確認する (= Step 1 後のセッションでまだ確認できていない項目)。

1. **AI ON で消しゴム → AI OFF**: AI off に切り替えても upscale 残らないこと
2. **preview 押下 → 押下中に shape 移動 → 再 preview**: 古い結果が一瞬出ないこと
3. **AI 後完了 → 消しゴム preview 押下**: クラッシュしないこと (= R5 P1)
4. **隠蔽 preview ON で楕円ドラッグ**: マスク overlay がリアルタイム更新
5. **見開きで消しゴム入場 → ESC**: 元の見開きに戻ること
6. **補正 slider 操作中に消しゴム入場**: drag session が破棄されること
7. **ZIP / PDF 画像 でも上記がすべて動くこと**

### Perf チェック

```bash
# perf-log を取って analyze
bash scripts/perf_smoke.sh
python scripts/analyze_perf.py hitches  # 16ms 超のフレーム間隔
```

リファクタ前後でヒッチ数 + nav latency が悪化していないこと。
4K RGBA 32MB upload を毎フレ行う drag 中は p99 で 50-80ms 程度を許容
(= R5 でリアルタイム化したときの想定値)。

## ステップ分割 (= コミット粒度)

各ステップ完了時に `cargo build --release` + `cargo test --lib` + `cargo fmt --check`
を通す。各ステップで **Claude にレビュー依頼** (= invalidate 漏れの別目チェック)。

### Step 2.1: generation 機構の導入 (非破壊)

- `App` に `input_generation` / `erase_mask_generation` / `conceal_mask_generation`
  HashMap を追加 (空でスタート)
- 既存の `clear_*` メソッド呼び出し箇所で generation を bump するように改造
- 既存 cache key は変えない (= 既存挙動を維持しつつ、generation 情報を持つだけ)
- これでまだ何も直らないが、次ステップの土台ができる

### Step 2.2: `EraseResultKey` + `erase_result_cache` 新設

- 新 cache + key を追加
- `apply_inpaint_only` / `apply_inpaint_result` を改造して `fs_cache` ではなく
  `erase_result_cache` に書き込む
- 表示パイプライン (= `resolve_display_texture` 相当) で
  `erase_result_cache > adjustment_cache > ai_upscale_cache > fs_cache` の順に
  チェックするように変更
- `fs_cache` への書き戻しを完全に除去

### Step 2.3: `mutate_erase_mask` 中央集約

- `EraseMaskState` 構造体を定義 (= mask + shapes + selected_shape の借用ハンドル)
- 既存の `self.erase_mask = ...` / `self.erase_shapes.push(...)` 等の直接 mutation
  を全部 `mutate_erase_mask` 経由に置換
- 関数内で自動 generation bump + `erase_preview_cache.remove`
- 手動 `clear_erase_preview()` の呼び出しを削除可能なものから順に削除

### Step 2.4: 表示パイプラインの enum 化

- `DisplayLayer` enum を定義
- `resolve_display_texture` メソッドを実装
- 既存の絡まった if/let chain を新メソッド呼び出しに置換
- ui_fullscreen.rs の表示テクスチャ選択コードを 1 箇所にまとめる

### Step 2.5: pending key を `(idx, kind)` 化

- `EraseInpaintPending` map のキーを `usize` から `(usize, PendingKind)` に変更
- preview と commit が独立 cancel 可能になる
- Codex R5 P2 後段の指摘解消

### Step 2.6: ドキュメント更新 + 動作確認

- `docs/display-pipeline.md` の優先度表を新仕様に更新
- `docs/preset-and-adjustment.md` の cache 無効化ルールを更新
- `docs/async-architecture.md` の pending pattern 例を更新
- CLAUDE.md の対応する節を 1 箇所だけ更新 (= 永続データはスキーマ無変更なので
  マイグレーション不要、コミットメッセージに「リファクタのみ、永続データ無変更」
  と明記)

## 永続データへの影響

このリファクタは **メモリ上の cache 層**だけを触る。以下はすべて無変更:

- SQLite DB スキーマ (mask_db / conceal_db / adjustment_db / settings_db)
- サイドカーファイル (= mask `.json`, conceal `.json`)
- 設定ファイル (settings.db)
- 画像ファイル本体

CLAUDE.md「永続データ・スキーマ変更時の判断」セクション参照: 今回は永続データ
無変更なのでマイグレーション不要。

## 想定工数

- Step 2.1: 2-3 時間 (generation 機構導入)
- Step 2.2: 3-4 時間 (erase_result_cache 新設 + 表示順整理)
- Step 2.3: 4-5 時間 (mutate 中央集約、全 mutation 経路の置換)
- Step 2.4: 2-3 時間 (enum + chain 化)
- Step 2.5: 1-2 時間 (pending key 拡張)
- Step 2.6: 1 時間 (ドキュメント更新)

合計 13-18 時間 (= 半日〜2 日)。各ステップは独立に commit + テスト + Claude review。

## 参考ドキュメント

- `docs/display-pipeline.md` — 現在の表示テクスチャ優先順位 + 変換合成順序
- `docs/preset-and-adjustment.md` — 補正 / AI / プリセットのキャッシュ無効化ルール
- `docs/async-architecture.md` — ワーカー / 共有 atomic / キャンセル規約 (= pending
  pattern の前例)
- `CLAUDE.md` — プロジェクト全体規約 (フォーマット / ドキュメント同時更新 / etc.)

## レビュー時の観点 (Claude 用)

このリファクタを Claude が review するとき、以下を必ずチェック:

1. **`fs_cache` への書き込み箇所が `thumb_loader` / `fs_loader` だけになっているか**
   (= raw decode 経路以外から書いていない)
2. **`self.erase_shapes` / `self.erase_mask` の直接 mutation が `mutate_erase_mask`
   経由以外に残っていないか**
   - `git grep 'erase_shapes\.' src/` で確認
3. **`clear_erase_preview()` の呼び出しが Step 2.3 後に最低限 (= モード退出時等)
   に削減されているか**
4. **`EraseResultKey` の `input_gen` / `mask_gen` 更新点が全部網羅されているか**
   - AI 完了 / adjustment 変更 / mask 変更 の 3 経路
5. **解像度ミスマッチの assert / fallback がリファクタ後も健在か** (= 入力 pixels と
   composite mask の `[w, h]` 一致確認)
6. **見開き / ZIP / PDF / 動画フォルダで既存挙動が壊れていないか** (= 統合テストか
   手動テスト)
7. **CLAUDE.md「コード修正時のドキュメント同時更新」に従って関連 docs が更新済みか**

---

このブリーフを Codex GUI セッションで開いて、step 2.1 から順に着手してください。
各ステップ完了時に Claude に commit を渡して review を依頼すると invalidate 漏れの
別目チェックができます。
