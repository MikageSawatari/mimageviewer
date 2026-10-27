# epaint 0.37.0 のローカル変更（§1.443）

## 原本

- crates.io `epaint` 0.37.0 の registry 展開物を実ファイルで複製した。
  Cargo キャッシュのソースは編集していない。
- archive SHA-256: `97bac86ae3dde93fc3951e790951caea739ff39a6c89d909cdaf1d9278dbb1bd`
- `.cargo_vcs_info.json`: `emilk/egui` commit
  `7c0b40402271e8d40cda69f87cf3c2617c2e4382`、`crates/epaint`。
- `Cargo.toml.orig`、README、原本の standalone `Cargo.lock`、bench を保持。
  正規化済み `Cargo.toml` に既存 vendor と同じ独立 `[workspace]` だけを追加する。
  registry の完了印 `.cargo-ok` は同梱しない。

## 差分と理由

| 項目 | 分類 | 実装・根拠 |
| --- | --- | --- |
| 単色 bitmap のカラー判定 | (b・新規) v4.8.0 回帰修正 | `src/text/font_face.rs::FontCell::has_color_glyph` で Skrifa の `BitmapData::Mask` を除外する。EBDT の有無や画素の白さで色を推定しない。単色文字を通常の hinted/subpixel outline 経路へ渡し、`is_color: false` とテーマ・選択・無効状態の文字色を復元する |
| カラー字形 | (a) 本家を維持 | COLR、PNG/BGRA のカラー判定、`color_fonts` feature を維持する。フォント候補・fallback・tessellator は変更しない |
| Windows フォント回帰 | (b・新規) | `font_color_tests.rs`。7該当 face、Meiryo 対照、Segoe UI Emoji。字形の色分類・実画素・mesh 頂点色・override 色を5サイズ×3倍率で検査。フォント未導入は理由付きでスキップする。Windows + `color_fonts` で実行し、実アプリは起動しない |

vello_cpu 0.2.0 が使う glifo 0.3.0 は `BitmapData::Mask` を描かず outline へ進む。
従来のカラー経路も実際には白い outline を描いていたため、本修正で embedded mask の描画を
削除するものではない。epaint の通常 outline 経路で文字色を適用する。
新しい状態・worker・待機・画素判定は追加しない。

## 上流照合（2026-10-25）

[上流 main の font_face.rs](https://github.com/emilk/egui/blob/main/crates/epaint/src/text/font_face.rs)
（取得時 file blob `3f0374287272f3b17c91234d9b11b90ba61c25dc`）にも
`glyph_for_size(...).is_some()` が残っていた。
PR の `bitmap color` / `Mask glyph`、issue の `EBDT` / `font white` 検索では、
この不具合の修正は見つからなかった（検索範囲での確認）。
導入元は [#8492](https://github.com/emilk/egui/pull/8492)。
上流が単色マスクを同様に扱うようになったらローカル差分の撤去を検討する。

## 検証

```powershell
cargo test --manifest-path vendor/epaint/Cargo.toml --lib --features default_fonts,color_fonts
```

`default_fonts` は原本の lib テストにも必要。feature なしでは原本の
`text_layout.rs::unique_test_key` の cfg 不整合でコンパイルできない。
この既存テストの feature 問題は §1.443 の修正には含めない。
修正前7件失敗・修正後104件成功。本体 gate と残る検証の台帳は
[§1.443 検証記録](../../docs/section443-font-color-fix.md) を参照。

## ライセンス

宣言 `MIT OR Apache-2.0` を維持。原本 archive に本文がないため、既存 `vendor/egui` の
同名本文を byte 変更なしで同梱する。

| コピー元 | SHA-256 |
| --- | --- |
| `../egui/LICENSE-MIT` | `3fceb7b317f3c70451942b0e123723a2e250c7eaf970ce4b490a57a94c5588ff` |
| `../egui/LICENSE-APACHE` | `1ac50a5abd5cff5331d59928d83369b66b0099a88f0111dc2175526b3c666dec` |
