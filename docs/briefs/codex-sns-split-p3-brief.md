# Codex ブリーフ — SNS 分割書き出し P3 (書き出し)

正本: [docs/sns-split-export-plan.md](../sns-split-export-plan.md) の **§4.6**。
P1 = `676b4cbd`、P2 = `b0b55567` で入っています。

このブリーフの範囲は **P3 のみ**。**投稿後プレビュー (P4) はやらない。**

---

## 1. 作るもの

SNS 分割モードで <kbd>Ctrl+E</kbd> (`KeyAction::SnsSplitExecute`) を押したら、
**N 枚を連番で書き出す**。現在は [ui_sns_split.rs](../../src/ui_sns_split.rs) の
`handle_sns_split_keys` がモードを抜けるだけになっているので、そこを繋ぐ。

既存のエクスポートダイアログを経由させる (出力先・形式・倍率・連番命名・メタデータの扱いを
再実装しないため)。**1 スナップショットから N ファイルを書く仕組みは既にあります。**

---

## 2. 触ってよいファイル

- `src/export_dialog.rs`
- `src/ui_fullscreen.rs` (エクスポートダイアログの UI と request 組み立て)
- `src/ui_sns_split.rs`
- 必要なら `src/sns_split.rs` (公開 API の追加のみ。既存の幾何の意味は変えない)

**`src/export_crop.rs` と `src/ui_crop.rs` は変更しない。**
既存の「切り取り」からのエクスポートの挙動を 1 ミリも変えないこと。

---

## 3. 仕組み

### 3.1 枠ごとの crop を `ExportEntry` に持たせる

現状:

```rust
// export_dialog.rs:143
pub struct ExportEntry {
    pub label: String,
    pub suffix: u8,
    pub conceal_preset: Option<ConcealPreset>,
}
```

`ExportRequest.entries` をワーカーが回して 1 エントリ 1 ファイル書きます
(隠蔽プリセット 5 種の一括書き出しがこの仕組み)。ここに枠の crop を足す。

```rust
pub struct ExportEntry {
    pub label: String,
    pub suffix: u8,
    pub conceal_preset: Option<ConcealPreset>,
    /// SNS 分割の枠。`Some` なら `ExportPagePixels.crop` より優先する
    pub crop: Option<crate::export_crop::CropRect>,
}
```

`render_export_page_pixels` ([export_dialog.rs:530](../../src/export_dialog.rs:530)) が
`ExportPagePixels.crop` を適用しているので、**エントリ側の crop があればそちらを使う**。
crop はパイプライン最終段 (回転より前) で、そこは変えない。

既存の呼び出し全部で `crop: None` を足すことになります。**`Default` を生やして省略できる
ようにしない**でください。新しいエントリを作るときに「この書き出しの crop は何か」を必ず
考える形にしたいので、フィールドは明示で埋める。

### 3.2 ダイアログを分割モードで開く

`handle_sns_split_keys` で `SnsSplitExecute` を受けたら、既存の切り取りと同型にする
([ui_crop.rs:206](../../src/ui_crop.rs:206) 参照)。

1. **抜ける前に `layout.frames()` を取っておく** (reset すると layout が消える)
2. `fits()` が false なら**書き出さず、モードも抜けない**。パネルの警告を出したままにする
3. `reset_sns_split_mode()` してから `open_export_dialog_for_current`
   ([ui_fullscreen.rs:34481](../../src/ui_fullscreen.rs:34481)) を呼ぶ
4. 取っておいた枠を `ExportDialogState` へ載せる

`ExportDialogState` に足すもの:

```rust
/// SNS 分割から開いたときの枠。空なら通常のエクスポート
pub sns_split_frames: Vec<crate::export_crop::CropRect>,
```

### 3.3 entries の組み立て

`start_export_from_dialog` ([ui_fullscreen.rs:35105](../../src/ui_fullscreen.rs:35105)) の
entries 組み立て ([同 35164](../../src/ui_fullscreen.rs:35164)) を分岐させる。

- `sns_split_frames` が空でない場合:
  - **entries は枠の数だけ**。`suffix` = 1..N、`label` = `"1 / 4"` 形式、
    `conceal_preset` = `None`、`crop` = 対応する枠
  - `resolve_session_basename` に渡す suffixes も 1..N
- 空の場合は今までどおり (隠蔽プリセットの選択に従う)

### 3.4 隠蔽プリセットの一括書き出しは無効にする

正本 §4.6 のとおり。**N x 5 = 最大 20 ファイルは意図と合わない。**

- ダイアログの「現在の設定 (_0)」「プリセット1〜5」のチェックボックス
  ([ui_fullscreen.rs:34857](../../src/ui_fullscreen.rs:34857) 付近) は、SNS 分割から
  開いたときは**無効表示にし、理由をツールチップで出す**
  (例「SNS 分割では現在の設定のみ書き出します」)
- **黙って消さない。**利用者は「プリセットも一緒に出る」と思っている可能性があるので、
  理由が見える方がよい (CLAUDE.md の編集用ツール無効化と同じ考え方)
- 永続化される `selection` を SNS 分割の都合で書き換えないこと。既存の
  `original_selection` の扱い ([ui_fullscreen.rs:35180](../../src/ui_fullscreen.rs:35180)
  付近のコメント参照) を壊さない

### 3.5 その他

- **倍率の既定は等倍**。既存の選択肢は残す (正本 §4.6)
- ダイアログの見出しか説明に「4 枚に分割して書き出します」相当を出し、何が起きるか分かるように
- 出力サイズ表示は**枠 1 枚の寸法**にする (合成後ではなく)

---

## 4. 検証

```
cargo fmt
python scripts/check_ui_glyphs.py
cargo test -p mimageviewer --lib
cargo test --test ui_snapshot
cargo check -p mimageviewer --bin mimageviewer-core
```

テストは最低限これらを足す:

- `ExportEntry.crop` が `ExportPagePixels.crop` より優先される
  (`render_export_page_pixels_applies_crop_last` ([export_dialog.rs:739](../../src/export_dialog.rs:739))
  の隣に、同じ流儀で)
- 枠 N 個から entries が N 個できて、suffix が 1..N になる
- SNS 分割から開いたときは隠蔽プリセットのエントリが増えない
- `fits()` が false のとき `SnsSplitExecute` で書き出しに進まず、モードも抜けない
- 通常のエクスポート (SNS 分割を経由しない) の entries が今までと同じ

**既存の 6869 件を落とさないこと。**落ちたら、なぜその期待値が変わるのかを報告してください
(勝手に期待値だけ書き換えない)。

`cargo test --test ui_snapshot` が落ちたら、ダイアログの見た目変化が意図どおりかを確認し、
意図どおりなら `UPDATE_SNAPSHOTS=1` で更新して報告する。

作業後は `.\scripts\build-dev.ps1` で実機確認用バイナリを作る。コミットはしない。

## 5. 報告してほしいこと

1. `ExportEntry.crop` を足したことで `crop: None` を書いた箇所の一覧
2. 隠蔽プリセットを無効化した方法と、`selection` の永続化を壊していない根拠
3. `fits()` が false のときの挙動
4. 追加したテストと全コマンドの結果
5. 正本と食い違った点、迷った点 (正本は書き換えない)
