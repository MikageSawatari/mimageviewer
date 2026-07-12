# 本として読むときのページ順を一覧ソートから分離する (§1.14)

最終更新: 2026-07-12 / 状態: 設計確定・未実装 (実装は Codex Sol、検収 ClaudeCode)

## 1. 背景・問題

出典: mImageViewer 専用スレ 47。

一覧 (サムネイル整理) 用のソートを日付順などにしていると、ZIP/対応アーカイブを「本」として
ページ送りするときも同じ順序になり、読書用途では不自然になる。読書時はファイル名順で
ページが進む方が自然。

現状の実装 (調査済み 2026-07-12):

- ページ順序は `self.items` 配列の順序そのもの。ページ送り・見開き・連結読みはすべて
  この配列を参照する。
- ZIP 系の `items` は **一覧ソート `settings.sort_order` を直接使って**並べている
  (`finalize_zip_enumerate` の `pending.sort_order`、`zip_nav_show_current_level` /
  `zip_nav_dfs_fullscreen` の `self.settings.sort_order`)。`book_sort_order_for_path` を
  通っていない。→ **これが §1.14 の不満の本体**。
- `book_sort_order_for_path(path)` は現状、製本した本 (`%Pictures%/mimageviewer/books/<名前>/`
  直下フォルダ) だけ `Numeric` 固定、それ以外は `settings.sort_order` を返す。

## 2. 調査で判明した「実際に修正が必要な範囲は狭い」根拠

1. **既定の一覧ソートはすでに `SortOrder::FileName` (名前順)** (`settings.rs` の
   `enum SortOrder { #[default] FileName, .. }`)。既定設定のユーザーは現状でも名前順で
   読んでいるので、今回の変更で挙動は変わらない。影響を受けるのは一覧を日付順に
   している利用者だけで、その人にとっては ZIP を名前順で読めるのは求めていた改善。
2. **PDF はソートを一切通さない**。`poll_pdf_enumerate` は enumerate 順 (= ページ番号
   昇順) で `GridItem::PdfPage` を push し、`arrange_grid_items` にもソートにも掛けない。
   よって PDF のページ順は元からページ番号順で固定。**修正不要**。
3. **画像フォルダは既存の「画像のみのフォルダを、PDF/ZIP のように本として扱う」チェックが
   escape hatch**。写真を日付順で見たい人はチェックを外せば通常フォルダ = 一覧順のまま。

→ 実効的に「一覧を日付順にした利用者が ZIP 系を読むとき」だけに効く、ローリスクな改善。

## 3. 確定方針

- **本として読むときのページ順を、一覧ソートから切り離して名前順 (`SortOrder::FileName`,
  Windows に近い自然順) で固定する。**
- **専用設定は追加しない** (fixed)。理由:
  - 既定ユーザー (一覧ソート = FileName) は無影響。日付ソート利用者にとっては純粋な改善。
  - 画像フォルダは「本扱い」チェックで既に一覧順へ戻せる。
  - per-book 上書き・日付順オプションは需要が不明で、入れると設定が増えるだけ。将来
    「ZIP を一覧順のまま読みたい」需要が実際に出たら、`FollowList / Name` の 2 択トグルを
    後から足すのは数行 (enum + serde default + resolver 分岐 + ラジオ 2 個) で可能。
- **`SortOrder::FileName` を採用**。§1.14 が指定する「Windows に近い自然順」であり、
  `Numeric` (番号順・区切り無視) とは別物。

### 適用範囲

| 対象 | 変更前 | 変更後 |
| --- | --- | --- |
| ZIP / CBZ / 直接閲覧 RAR/CBR / 変換キャッシュ (7z/LZH/RAR→ZIP) | 一覧ソート | **名前順固定 (FileName)** |
| 画像のみフォルダ (「本扱い」= `auto_fullscreen_image_folders` ON) | 一覧ソート | **名前順固定** |
| PDF | ページ番号順 (元から) | 変更なし |
| 製本した本 (`is_direct_book_folder`) | Numeric (元から) | 変更なし |
| 通常フォルダ browse / 画像フォルダ「本扱い」OFF | 一覧ソート | 変更なし (写真の日付順運用はここ) |
| サブフォルダ展開 / ファイル名スタック / Ctrl+G 検索 / ★一覧 | 各ビュー固有の順 | 変更なし (結果順に意味あり) |

## 4. 実装マップ (修正が必要な経路)

読書ページ順を返す固定値を 1 箇所に定義し、ZIP 系と画像フォルダ本の sort source を
そこへ差し替える。**PDF・製本本・通常フォルダ・展開/スタック/検索/★一覧は触らない。**

推奨: 定数 or ヘルパーを 1 つ用意する。

```rust
/// ZIP/CBZ/直接閲覧RAR/変換キャッシュ、および「本扱い」ON の画像のみフォルダを
/// 「本」として読むときのページ順。一覧の整理用ソート (settings.sort_order) から
/// 独立した固定値 (名前順 = Windows に近い自然順)。§1.14。
pub(crate) const BOOK_READING_PAGE_ORDER: crate::settings::SortOrder =
    crate::settings::SortOrder::FileName;
```

差し替え対象 (すべて `src/app.rs`、いずれも現在は一覧ソート直参照):

1. **`finalize_zip_enumerate`** — `let sort = pending.sort_order;` (~16059) を
   `BOOK_READING_PAGE_ORDER` に。ここが ZIP/直接閲覧RAR/変換キャッシュの本を開いた
   ときの L2 ページ一覧 + L3 読書の初期並び。`materialize_current(sort)` と直後の
   `arrange_grid_items(.., sort)` の両方に効く。
   - `pending.sort_order` を capture している箇所 (~16009 の同期 fallback,
     ~16017 の async pending 生成) は本読みに使う必要がなくなるが、フィールド自体を
     残すか除去するかは Codex 判断 (他用途が無ければ finalize 側で定数参照に統一するのが簡潔)。
2. **`zip_nav_show_current_level`** — `let sort = self.settings.sort_order;` (~16305) を
   定数に。ネスト ZIP の階層移動 (enter/back) 後の再 materialize。
3. **`zip_nav_dfs_fullscreen`** — `let sort = self.settings.sort_order;` (~16612) を
   定数に。本またぎ DFS。
   - `zip_nav_dfs_step` など他に `self.settings.sort_order` を使う zip_nav 経路が
     あれば同様に統一する (grep `self.settings.sort_order` で zip_nav 系を洗う)。
4. **`load_folder_with_scan`** — 画像のみフォルダを本扱いする場合だけ media ブロックを
   名前順にする。現状 `let sort = self.book_sort_order_for_path(&path);` (~13514) で
   folders/all_media を並べている。この関数は scan 済みで image-only 判定可能なので、
   「`folders` が空 && `all_media` が全て画像 (動画/音声なし) && `auto_fullscreen_image_folders`
   が実効 ON」なら sort を `BOOK_READING_PAGE_ORDER` にする。それ以外は従来どおり
   `book_sort_order_for_path(&path)` (= 製本本は Numeric、通常フォルダは一覧ソート)。
   - `auto_fullscreen_image_folders` の実効値は `Settings::auto_fullscreen_image_folders_enabled()`
     を使う (`effective_auto_fullscreen_zip_pdf() && auto_fullscreen_image_folders`)。
   - 画像のみ本扱いのとき、folders は空なので media ブロックだけ名前順にすれば足りる。

### 触ってはいけない (回帰防止)

- PDF (`poll_pdf_enumerate`): ソート非経由のまま。
- 製本した本: `book_sort_order_for_path` の `is_direct_book_folder → Numeric` を維持
  (ゼロ埋め名なので FileName と実質同値だが、既存挙動を触らない)。
- 通常フォルダの folders ブロック (Folder/ZipFile/PdfFile/ConvertibleArchive) の並び:
  一覧ソート維持 (Explorer 風の 2 段構成 = §1.6 の grid_display_order は不変)。
- サブフォルダ展開 (`subfolder_expansion.rs`) / ファイル名スタック (`filename_stack_ui.rs`) /
  レーティング一覧 / Ctrl+G 検索 flat result: すべて現状維持。

## 5. 整合性

`items` 配列を enumerate/materialize 時に 1 回名前順で作るだけなので、下流はすべて
その配列に追随して整合する:

- **見開きペアリング / 綴じ方向 (RTL/LTR) / 見開き 1 ページずらし / 連結読み**:
  `self.items` から計算されるので新しい順序に自動追随。RTL/spread モードは本キー
  単位の設定 (`spread.db`) で index 非依存 → 影響なし。
- **回転 / 補正 / レーティング / タグ / マスク / テキスト / 切り取り**: すべてページ
  パスキー (`ZipImage.zip_path::entry_name` 等) 単位で保存 → 並び替えに非依存。
- **読書位置 (`book_resume.db`)**: コンテナパス + **ページ index** で保存。順序を一覧
  ソートから切り離すと、日付ソート利用者では変換直後の 1 回だけ復元位置が旧 index の
  別ページを指し得る。無効 index はフォールバック済み (`book_resume_db.rs` doc)。以後は
  むしろ「一覧ソートを変えても読書位置がズレない」= 安定方向。既定ユーザー (FileName) は
  index の意味が変わらないので無影響。
- **Ctrl+↑↓ フォルダ横断 / 兄弟移動**: `folder_tree` の DFS を使い `items` 順に非依存 →
  影響なし。移動先で先頭 image-like を開く仕様 (`fullscreen-navigation-consistency.md` §3.2)
  も、その「先頭」が名前順の先頭になるだけで整合。
- **ネスト ZIP の ZipDir 代表サムネ**: `materialize_current(sort)` は sort で部分木代表を
  選ぶため、日付ソート利用者では代表が変わり ZipDir の cache key (`representative` 込み) が
  変わって一度だけ再生成される。既定ユーザー (FileName) は不変。サムネは再生成可能物なので許容。

いずれの副作用も **日付/番号ソート利用者のみ・一度きり・データ破壊なし**。既定 (FileName)
ユーザーは完全に無影響。

## 6. 既定変更の告知

- 既定の読書順が (日付ソート利用者にとって) 変わるため、`src/version_highlights.rs` の
  `TABLE` に今回バージョンの `must_read` として 1 行足す。display-only・内部用語なし。
  例: 「ZIP や対応アーカイブを本として読むときのページ順を、一覧の並び替え設定と切り離して
  ファイル名順にしました (画像のみのフォルダを本として扱う場合も同様)」。
- 永続スキーマ変更なし (新設定フィールドなし)。マイグレーション不要。`book_resume.db` の
  既存 index はそのまま流用 (上記フォールバックで吸収)。

## 7. 非対応 / 将来 (意図的に入れない)

- **per-book のページ順上書き** (例: 写真 ZIP だけ日付順): 需要不明。写真は通常フォルダ or
  「本扱い」OFF で一覧順運用できるため見送り。入れるなら既存の本単位状態 (spread/RTL/resume/
  view_trim と同じ本キー) に載せる。
- **グローバルな日付順オプション**: 漫画含む全部の本に効いてしまうため不適。
- **グローバル 2 択トグル (一覧順 / 名前順)**: 現時点では不要 (既定ユーザー無影響 + 画像フォルダ
  は既存チェックで代替)。「ZIP を一覧順のまま読みたい」需要が実際に出たら後から追加する。

## 8. テスト

- **純ロジック unit**: 混在名 (`10.jpg`, `2.jpg`, `1.jpg`, `p03.png` 等) を含む ZIP 相当の
  items を、一覧ソート = `DateDesc` の状態で本として並べると **名前順 (FileName)** になること。
  一覧ソート = `DateDesc` の通常フォルダ browse は従来どおり日付順のままであること (回帰防止)。
- 画像のみフォルダ: `auto_fullscreen_image_folders` ON なら名前順、OFF なら一覧ソート。
- 製本した本 (`is_direct_book_folder`) は従来どおり Numeric (`app/tests.rs` の
  `book_sort_order_for_path` 既存テストを壊さない)。
- PDF ページは page_num 昇順のまま (ソート非経由の確認)。
- `cargo test --bin mimageviewer-core` 緑 + `cargo fmt` + `python scripts/check_ui_glyphs.py` 0 件。
- 実機スモーク: 一覧を日付順にした状態で ① 名前が数字連番の ZIP を開き、L2 ページ一覧と
  ページ送りが名前順になる ② PDF が従来どおりページ順 ③ 画像のみフォルダを「本扱い」ON で
  開くと名前順、OFF で一覧(日付)順 ④ 通常フォルダ browse は日付順のまま。

## 9. ドキュメント更新

- `docs/virtual-folders.md`: ZIP/PDF のページ順が一覧ソートから独立して名前順になる旨
  (§1 グリッド表示順の記述、または新節)。
- `docs/fullscreen-navigation-consistency.md`: 「移動先の先頭 image-like」の順序基準が
  本読みでは名前順である旨を必要なら追記。
- ユーザー向け: `htdocs/mimageviewer/manual/` の本・ビューア関連ページ (バージョンタグ・
  内部用語なし。「本として読むときはファイル名順でページが進みます」程度)。
- `docs/next-release-backlog.md` の §1.14 を削除 (実装完了時)。

## 10. Codex 実装ブリーフ (要約)

- `src/app.rs` に `BOOK_READING_PAGE_ORDER = SortOrder::FileName` を定義。
- §4 の 4 経路 (finalize_zip_enumerate / zip_nav_show_current_level / zip_nav_dfs_fullscreen /
  load_folder_with_scan の画像のみ本判定) を差し替え。PDF・製本本・通常フォルダ・展開/
  スタック/検索/★一覧は不変。
- §6 の version_highlights 1 行 + §9 のドキュメント。
- §8 のテスト。`cargo fmt` + `cargo test --bin mimageviewer-core` + glyph lint。
- Windows ネイティブ挙動 (フルスクリーン読書) を変えるので、実機依頼前に
  `.\scripts\build-release.ps1` で検証バイナリを用意 (CLAUDE.md「実機検証用バイナリの準備」)。
