# 補正レイヤー テスト方針

ラボ単体ツール (`tools/local_adjust_lab/`) → mIV 本体への結合作業で発生する **退行を
継続的に検知** するためのテスト方針。Codex/Claude Code が修正コミットを作るときは、
**修正と同じコミット内に最低 1 本の自動テスト**を追加することをルール化する。

## なぜテストを書くか

実機検証で振る舞いの差分が次々に出る (M-1〜M-7 のような UI レベルバグ) のは、
コード grep ベースの監査だけでは「**ある field が定義されているが使われていない**」
「**ある UX 修正がコミット後に reset された**」のような**振る舞い差**を検出できないため。
自動テストで振る舞いを符号化すれば、リファクタ / 再結合作業で同じバグが再発しない。

## テストの 3 レイヤー

| レイヤー | 場所 | 検証対象 | アクセス可能な型 |
|---|---|---|---|
| **(L1) コアロジック** | `crates/local-adjust-core/src/lib.rs` の `#[cfg(test)] mod` | mask 評価、effect 適用、core algorithm | 全ての pub 型 |
| **(L2) パネル関数 unit** | `src/ui_adjustment_panel.rs` の `#[cfg(test)] mod` (panel:180-) | mask preview alpha、ハンドル hit-test、ジオメトリ計算 | `pub(crate)` 含めて全部 |
| **(L3) integration parity** | `tests/local_adjust_core_parity.rs` 他 | ラボ既知値 (色 RGB、デフォルト、preset 値) と pub API の一致 | crate pub のみ |

**新規バグ修正には、L2 (panel 内 #[cfg(test)]) を最低 1 本添える** ことを Codex への
標準指示にする。M-N がすでに混じった panel の test mod (panel:180-547) に追加していく。

## どのバグをどこでテストすべきか

| バグ ID | レイヤー | テスト関数 (現状 + 追加) | コミット |
|---|---|---|---|
| M-1 全体マスク preview 非表示 | L2 | `full_mask_preview_hides_plain_full_base_but_shows_subtract_result` ✅ | 6770ad63 |
| M-2 削除マスク色分け | L2 | `subtract_override_edit_preview_uses_edit_color` ✅ + 追加で「Add も色違い」テストを書く | 6770ad63 |
| M-3 Rect/Ellipse ハンドル | L2 | `rect_and_ellipse_shape_handles_are_hit_testable` ✅ + drag apply のテスト追加 | 6770ad63 |
| M-4 グラデーション再ドラッグ保存 | L2 | **未追加** — Codex 修正と同時に `linear_gradient_initialized_canvas_click_does_not_reset` を書く | (Codex M-N 対応) |
| M-5 領域カラーアニメーション | L1+L3 | L3 `lab_animated_overlay_color_is_time_dependent` ✅ + L2 で mIV 側の boundary color が同じ式か検証 | core_parity.rs |
| M-6 隠蔽加工バイパス | App統合 | **難度高** — 別途 conceal_compose 周辺の unit test として書く必要あり | (TBD) |
| M-7 被写体マスク UI gating | L3 (値) + L2 (UI gating) | L3 `subject_refinement_default_is_disabled_with_lab_baseline` ✅ + `subject_refinement_three_preset_values_match_lab` ✅ + L2 で UI 描画が disabled になることを別途 | core_parity.rs + Codex 追加 |
| A-1 直線端の round cap | L2+L3 | L2 `line_shape_preview_uses_square_end_caps` ✅ + L3 `line_shape_has_square_end_caps` ✅ + L3 `line_shape_vertical_has_square_end_caps` / `line_shape_diagonal_has_square_end_caps` ✅ (方向別ガード) | ef750308 / P4-6 |
| A-2 アニメーション点滅が点 | L2 | L2 `mask_preview_overlay_at_2048_completes_in_30ms` ✅ (性能) + `mask_preview_max_texels_constant_stays_at_or_above_2048` ✅ (= 定数値ガード、768 への退行を検知) + `region_boundary_color_completes_meaningful_hue_rotation_in_one_second` ✅ (= アニメーション周波数ガード) | ef750308 / P4-7 |
| A-3 Ctrl+Shift modifier 検出失敗 | (テスト困難) | OS API (`GetAsyncKeyState`) 経路は unit test 不可。代わりに L3 `bypass_and_prefix_preview_caches_are_separate_lanes` ✅ で **cache キーが衝突しないこと** を符号化 | adbc3dab / P4-1 |
| A-3 v3 prefix と bypass の取り違え | L2+L3 | `local_adjust_layer_bypass_disables_only_selected_layer` ✅ + `local_adjust_layer_bypass_matches_lab_transformation` ✅ (= ラボ変換式との semantics 一致) + `local_adjust_prefix_preview_boundaries` ✅ (= 直前まで preview の境界条件) | adbc3dab / P4-2, P4-3 |
| bypass preview cache 共存 | L3 | `bypass_preview_cache_coexists_with_final_composite_cache` ✅ (= final composite を巻き込まずに toggle 可能であることのガード) | P4-5 |
| bypass / prefix worker cancel | L3 | `clear_local_adjust_caches_cancels_bypass_and_prefix_pending` ✅ + `clear_local_adjust_caches_for_other_idx_keeps_bypass_pending` ✅ + `poll_bypass_preview_discards_stale_ready_result` ✅ (= stale write 防止) | P4-8 |
| bypass で残るレイヤーが無いケース | L3 | `local_adjust_layer_bypass_returns_none_when_no_other_enabled_layers_remain` ✅ (= worker 起動最適化、無効レイヤー / opacity=0 / 範囲外 idx 全部 None) | P4-4 |

## Codex への指示テンプレート (修正コミット時)

```
[修正内容] [M-N に対応する fix を書く]

検証:
1. cargo check --workspace
2. cargo test --lib [追加した test 名]
3. cargo test 全体

このコミットに以下のテストを追加してください:

inline (src/ui_adjustment_panel.rs の #[cfg(test)] mod local_adjust_segmentation_tests に):
- `[テスト関数名]`: [振る舞い記述]
  - 期待: [具体的な assertion]

integration (tests/local_adjust_core_parity.rs に該当する場合):
- `[テスト関数名]`: [core スペック検証]

テスト無しの修正は受け付けません。テストが書けない場合 (= App 状態が必要) は
その理由を本文に明記してください (= 後で integration test として補完する候補に挙げる)。
```

## ラボ既知値 (定数スペック)

Codex が修正で値を書く際の参照。**ラボの値を一字一句揃える** ことが基本方針。

### マスクプレビューの透明度定数
- `MASK_PREVIEW_BASE_ALPHA = 155.0` (`f32`、base 色のアルファ上限) — ラボ
  `tools/local_adjust_lab/src/main.rs` (`const` の方)
- `MASK_PREVIEW_EDIT_ALPHA = 225` (`u8`、edit 色のアルファ固定値) — 同上
- mIV 側に同じ定数名 + 値で持つこと (現在: `LOCAL_ADJUST_MASK_PREVIEW_BASE_ALPHA` /
  `LOCAL_ADJUST_MASK_PREVIEW_EDIT_ALPHA`)

### MaskColorPreset の RGB 三色 (PinkCyan / CyanOrange / YellowViolet)
それぞれ `(base_rgb, edit_rgb, boundary_rgb)`:
- **PinkCyan** (`label="1"`): `[255, 48, 84]` / `[64, 190, 255]` / `[255, 245, 120]`
- **CyanOrange** (`label="2"`): `[0, 205, 255]` / `[255, 150, 40]` / `[255, 235, 80]`
- **YellowViolet** (`label="3"`): `[255, 225, 40]` / `[185, 115, 255]` / `[80, 230, 255]`
- ラボ参照: `tools/local_adjust_lab/src/main.rs:937-957` の `impl MaskColorPreset::colors`
- mIV 側: `src/app.rs:415-444` の `impl LocalAdjustMaskColorPreset::colors`

### SubjectMaskRefinement デフォルト + 3 プリセット
- デフォルト: `enabled=false, threshold=0.52, expand_px=0, feather_px=1`
- プリセット「標準」: `enabled=true, threshold=0.52, expand_px=0, feather_px=1`
- プリセット「硬め」: `enabled=true, threshold=0.58, expand_px=-1, feather_px=0`
- プリセット「柔らかめ」: `enabled=true, threshold=0.45, expand_px=0, feather_px=2`
- ラボ参照: `tools/local_adjust_lab/src/main.rs:5953-5982`

### Animated overlay color (M-5 領域境界アニメーション)
```rust
// ラボ tools/local_adjust_lab/src/main.rs:283-290
fn animated_overlay_color(ctx: &egui::Context, alpha: u8) -> Color32 {
    let t = ctx.input(|i| i.time);
    let phase = ((t * 3.0).sin() * 0.5 + 0.5) as f32;
    let r = 255_u8;
    let g = (72.0 + 168.0 * phase).round() as u8;
    let b = (220.0 - 156.0 * phase).round() as u8;
    Color32::from_rgba_unmultiplied(r, g, b, alpha)
}
```
mIV 側に移植するときは式を一字違わずコピーすること。
`tests/local_adjust_core_parity.rs::lab_animated_overlay_color_is_time_dependent` が
時間依存性 + R/G/B の値域を検証する。

## L3 テストの仕組み

`tests/local_adjust_core_parity.rs` は **`local_adjust_core` crate の pub API しか
使わない**。理由:

- mIV 本体 (`src/app.rs`, `src/ui_adjustment_panel.rs`) の型は `pub(crate)` で
  `tests/` から直接見えない。
- ラボ spec の値だけを**定数として** test に焼いておけば、mIV 実装の値とズレた
  瞬間に対応する L2 unit test が落ちる。L3 はラボ spec の自己整合性 (色が
  ちゃんと違うかなど) だけを確認。

## ui_snapshot.rs との関係

既存 `tests/ui_snapshot.rs` は `egui_kittest::Harness` で `mimageviewer::` の pub 関数
だけを呼ぶ前提。補正レイヤーの UI 描画関数 (`draw_local_adjust_*`) は `pub(crate)` で
public ではないので、現状 ui_snapshot.rs からは触れない。

**今後 v1.2.0 で UI snapshot を本格化するなら**:

1. `src/local_adjust_test_api.rs` を新設し、`pub fn` で test API を提供
   (内部で `pub(crate)` 関数を呼ぶラッパー)
2. `src/lib.rs` に `pub mod local_adjust_test_api;` を追加
3. `tests/local_adjust_ui_snapshot.rs` から `mimageviewer::local_adjust_test_api::*` を
   呼んで egui_kittest スナップショットを撮る

このパス整備は時間予算 (v1.1.0 まで) では実施せず、v1.2.0 で着手予定。

## Phase 2 拡充 (2026-06、A-3 トリロジー後)

A-1〜A-3 と Pipeline P1-P10 を経て、以下の新規テストが追加されている (P4-1〜P4-8):

- **`src/app/tests.rs::pipeline_cache_refactor_tests`** (7 → 15 件):
  - `bypass_and_prefix_preview_caches_are_separate_lanes` — Ctrl+Shift bypass と
    panel checkbox prefix の cache キーが別レーンに乗ることを符号化。
  - `local_adjust_layer_bypass_matches_lab_transformation` — lab
    `layers_with_selected_layer_bypassed` (tools/local_adjust_lab/src/main.rs:23348)
    の変換式と mIV `App::local_adjust_layers_with_selected_layer_bypassed`
    の出力が並びレベルで一致することを毎 layer_idx で検証。
  - `local_adjust_prefix_preview_boundaries` — 「選択レイヤーまでプレビュー」が
    layer_count=0 / =len / >len で None を返す境界条件、=1/=2 で先頭から N 枚返す
    semantics を固定。
  - `local_adjust_layer_bypass_returns_none_when_no_other_enabled_layers_remain` —
    残りレイヤーが空 / disabled / opacity=0 / 範囲外 idx の全 None ケース。
  - `bypass_preview_cache_coexists_with_final_composite_cache` — Ctrl+Shift トグル時に
    final composite を捨てないことを assertion (= スライダー応答悪化を防ぐ)。
  - `clear_local_adjust_caches_cancels_bypass_and_prefix_pending` — 対象 idx の
    cache clear で bypass/prefix の両 pending の cancel が立つ。
  - `clear_local_adjust_caches_for_other_idx_keeps_bypass_pending` — 別 idx の
    clear ではこの idx の pending は無傷。
  - `poll_bypass_preview_discards_stale_ready_result` — pending が live でも、
    `result_key` が現状と違う Ready が届いたら cache に書かない。

- **`tests/local_adjust_core_parity.rs`** (15 → 17 件):
  - `line_shape_vertical_has_square_end_caps` — A-1 退行ガード (vertical)
  - `line_shape_diagonal_has_square_end_caps` — A-1 退行ガード (diagonal)

- **`src/ui_adjustment_panel.rs::local_adjust_segmentation_tests`** (新規 2 件):
  - `mask_preview_max_texels_constant_stays_at_or_above_2048` — A-2 退行ガード
    (定数 `LOCAL_ADJUST_MASK_PREVIEW_MAX_TEXELS` が 768 等に戻されたら fail)。
  - `region_boundary_color_completes_meaningful_hue_rotation_in_one_second` —
    A-2 関連ガード (= 1 秒で hue が十分回転することを RGB 差分で観測)。

A-3 の OS API 経路 (`GetAsyncKeyState`) は unit test できないため、**cache キー分離 +
変換式 parity** で間接的にカバーしている。Codex/Claude Code でこの周辺を修正する際は、
本表の右列のテストが残っているか確認すること (= 削除提案が来たら回帰防止根拠を必ず聞く)。

## Phase 3 拡充 (2026-06、P5 AI 先読み修復後)

P5-1〜P5-6 で AI 先読みパイプライン (edit → AI/補正の順序入れ替え) の退行を修復した
後、ついでに **`maybe_start_*_preview` 系の early-return guard** を符号化した。これらは
描画ループ内で毎フレーム呼ばれる経路なので、guard が外れると worker spawn 爆発や
無駄な cancel/respawn churn が起きる。

- **`src/app/tests.rs::pipeline_cache_refactor_tests`** (15 → 25 件):
  - `maybe_start_layer_bypass_returns_early_when_cache_already_present` —
    cache hit guard が外れたら毎フレーム spawn する退行を検知。
  - `maybe_start_layer_bypass_keeps_same_key_pending_alive` —
    同 key 重複要求で既存 pending を cancel/respawn する churn を防ぐ。
  - `maybe_start_layer_bypass_returns_early_when_no_remaining_enabled_layers` —
    `local_adjust_layers_with_selected_layer_bypassed` が None を返す全ケース
    (disabled / opacity=0 / 範囲外 idx / 残り 0 枚) で spawn しない最適化。
  - `maybe_start_layer_bypass_returns_early_when_source_unavailable` —
    mask page 編集中に bypass preview を要求しても worker spawn しない
    (= 編集確定前の不完全な source で render → flicker を防ぐ)。
  - `maybe_start_prefix_preview_returns_early_when_cache_already_present` —
    bypass 側と同型 (= 別 cache を触るので個別 fixation)。
  - `maybe_start_prefix_preview_keeps_same_key_pending_alive` — 同上。
  - `maybe_start_prefix_preview_returns_early_at_layer_count_boundaries` —
    `local_adjust_layers_until` の `count == 0 || count >= layers.len()` 境界
    (= 「先頭から 0 枚」「先頭から全枚」は元の合成と同じなので spawn 不要) を符号化。
  - `maybe_start_prefix_preview_returns_early_when_source_unavailable` — 同上。
  - `local_adjust_result_key_excludes_conceal_generation_m6` — **M-6 構造ガード**。
    `LocalAdjustResultKey` を exhaustive destructure してフィールド集合
    (idx / input_gen / erase_mask_gen / local_gen) を符号化。conceal_*_gen を
    追加する退行が入ると **コンパイル時** に検知される (= 補正レイヤー演算は
    conceal の上流であるという設計を構造で固定)。
  - `current_local_adjust_source_pixels_ignores_conceal_cache_m6` —
    **M-6 動作ガード**。conceal_cache に明確に識別可能な pixels を仕込んだ
    状態で `current_local_adjust_source_pixels` を呼び、戻り値が conceal の
    Arc と同一でないことを assert。compose chain が誤って conceal-applied
    pixels を補正レイヤーの入力源に回す退行を検知する。

これらの guard は src/app.rs 18957- (bypass) / 19007- (prefix) にある 4 つの
`is_some() return` + `let Some(..) else return` パターン、および
src/app.rs:18905- (`current_local_adjust_source_pixels`) の compose chain。
**`fn maybe_start_*` / `LocalAdjustResultKey` / `current_local_adjust_source_pixels`
をリファクタしたら必ず**この 10 本が green を保つことを確認すること。

### M-6 のテーブル更新

| バグ ID | レイヤー | テスト関数 | コミット |
|---|---|---|---|
| M-6 隠蔽加工バイパス | L3 (App統合) | `local_adjust_result_key_excludes_conceal_generation_m6` (構造) + `current_local_adjust_source_pixels_ignores_conceal_cache_m6` (動作) ✅ | (Phase 3) |

(冒頭の M-6 行が「難度高 / TBD」になっていたのを **Phase 3 で着手済み** と本表で
上書きしている。M-6 の完全カバー = compose chain 全分岐 (BypassLayer /
PrefixPreview / FullComposite / ShowSource × `ensure_final_composite_texture`
非経由) まで踏み込むのは L4 (egui_kittest による E2E) 待ちで、現状は
構造 + 動作の 2 軸ガードで実用上の退行は検知できる。)

## CI 統合

`.github/workflows/` の test job に何も追加せず動く (= `cargo test` 全体に含まれる)。
ローカルでは:
```bash
cargo test --test local_adjust_core_parity  # L3 のみ
cargo test --lib local_adjust                # L1 + L2 のみ
cargo test                                    # 全部
```

## 追加 / 改廃時のルール

- ラボ側で UI 仕様を変えた場合は、本ドキュメントの「ラボ既知値」セクションも合わせて
  更新する。値が乖離したまま放置すると L3 テストが見かけ通る = 退行検知が効かなくなる。
- mIV 側で `LocalAdjustMaskColorPreset` / `SubjectMaskRefinement` 等のデフォルトを
  変えるときは、L2 / L3 双方の関連テストを同時に修正する (機能変更 + テスト変更を
  同一コミットに収める)。
- 「lab spec と意図的に違える」場合は、対応する L3 テストにコメントで理由を明記し
  「lab とは違う仕様」と読めるようにする。
