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
