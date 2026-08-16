# 実装ブリーフ: 回転したページを見開きのペア判定に反映する

対象: v3.1.0。利用者報告 (2026-08-16)。

## 1. 症状

`d:\home\scan\comic\20250320_035.pdf` を開くと、**画面上は横長なのに見開きでペアになる**。
横長ページは単独表示になるはずだが、そうならない。

## 2. 原因 (調査済み・確定)

- PDF のページは 16 枚すべて **縦長** (503 × 727 pt、`/Rotate` なし、埋め込み画像も 4188 × 6055)。
- 利用者がこの PDF の**全ページに回転を保存**している (`rotation.db`: `page_0`〜`page_14` が 270、
  `page_15` が 90)。**表示は横長になる。**
- 回転は **描画時の GPU 行列**として適用され、テクスチャには焼かれない
  (`docs/display-pipeline.md` の表)。
- `is_landscape` (`src/ui_fullscreen.rs`) は fs_cache / thumbnail / page_dims のいずれも
  **回転前の寸法**を読み、回転を一切参照しない。よって縦長と判定してペアにする。

**初期実装からの問題** (`080198c8` の初版も `tex.size_vec2()` = 回転前)。v3.0.0 の
`7dbb2f39` は判定の入力を変えただけで、これを壊してはいない。

### 2.1 リモート閲覧も同じ食い違いを持つ

- リモートは配信画像に**回転を適用している** (`src/remote_ipc/thumbnail.rs` の `image.rotate90()` 等)。
- 一方ペアは `cached_landscape_flags` (`src/remote_ipc/container.rs`) が
  カタログの**回転前 source dims** から作る。
- **本体だけ直すと、同じ本が PC とブラウザで違うペアになる。** 両方直すこと。

## 3. やること

### 3.1 判定を 1 つの純関数に集約する

```rust
pub(crate) fn landscape_after_rotation(width: u32, height: u32, rotation: Rotation) -> bool
```

90 / 270 で幅高を入れ替えてから比較する。**本体もリモートもこの関数を通す。**
2 つの経路が別々に判定している限り、また食い違う。

### 3.2 本体

`is_landscape` に回転を渡す。ペア分けアルゴリズム
(`append_spread_display_units_until`) は**変更しない**。横長ページが単独ユニットになり
以降が 1 つずれる挙動は既存のままで、本物の横長ページで今日も起きている。

**借用の注意**: `App::get_rotation` は `&mut self` (memoize する)。
`build_spread_display_units_with_landscape` に渡すクロージャは `&self.fs_cache` 等を
借用済みなので、そのままでは呼べない。**ペア構築の前に nav 分の回転をまとめて取り、
スライスをクロージャへ渡す**。一括取得は `rotation_db::get_many` がある。

寸法が未確定のページは従来どおり縦長扱い (既存の doc comment の契約を維持)。

### 3.3 リモート

`cached_landscape_flags` で、カタログの寸法に回転を適用する。回転キーは
item 種別ごとの既存規則に従う (PDF ページは `<path>::page_N`)。
**`get_many` で一括読み**し、item ごとに DB を叩かない。

`build_remote_spread_page_groups` の signature は変えない (`is_landscape: &[bool]` のまま)。

### 3.4 通信は変えない (確認済み)

ペアは本体側だけで決まり、`PageGroup { anchor, pages }` を**アドレスで**送る。
クライアントはページ単位の cacheKey で要求し、組み替えは
`reanchorViewerPageGroups` と世代検証が既に吸収する。
**IPC のプロトコル版もペイロードの形も変えないこと。** 変える必要が出たら手を止めて報告する。

## 4. 確認と検証

- **回転を変えたらペアが組み直されるか。** 本体でページを R / L 回転したとき、
  そのページが単独になり以降が組み直されること。`rotation_cache` を clear している箇所と、
  ペアユニットの再構築契機を突き合わせて確認する。ここが今回いちばん怪しい。
- **PC とブラウザで同じ本が同じペアになるか。**

## 5. テスト

- `landscape_after_rotation` の単体テスト (0/90/180/270 × 縦横)。
- 既存のペア分けテストは `|idx| bool` のクロージャを取る形なので、
  「回転で横長になったページが単独ユニットになる」ケースを追加する
  (`spread_units_keep_leading_landscape_single_and_pair_following_pages` 等の隣)。
- リモート: `cached_landscape_flags` 相当に回転を反映するテスト。
- 実行: `cargo test -p mimageviewer --lib ui_fullscreen::`、
  `cargo test -p mimageviewer --lib remote_ipc::`、`cargo fmt --all`。

## 6. ドキュメント

- `docs/display-pipeline.md` — 回転とペア判定の関係を追記
  (「回転は描画時の行列」という記述の近くに、ペア判定も回転後で見ることを書く)。
- `docs/spec.md` — 見開きの横長単独表示が回転を含むことを追記。
- `htdocs/mimageviewer/manual/` — 見開きを説明しているページに一言。
  実装語・バージョン番号は書かない。

## 7. 対象外

- ペア分けアルゴリズムそのもの。
- 回転 UI・回転の保存形式。
- IPC プロトコルの変更。

## 8. 進め方

- `docs/next-release-backlog.md` は編集しないこと (別セッションが並行で編集している)。
- コミットは試みず、差分を作業ツリーに残すこと (こちらでレビューしてコミットする)。
- 範囲を超えると判断したら、症状パッチを入れずに報告する。
