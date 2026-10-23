# 本を優先するレーティング操作 (§1.417) 設計

- 状態: **ローカル master に統合済み (2026-10-23、利用者指示)。backlog §1.417 は統合時に削除したので、要求の正本は本書 §1 と下の経緯**。
  統合前の状態: **実装済み・独立コードレビュー受入可 (2026-10-23)・本体 lib 全件緑・隔離 data-dir で ClaudeCode が実アプリ確認 (下記)**。
  コードレビューは設計レビューと同じ担当 (SID 01a166e6…) が 2 回実施: 第 1 回は差し戻し (P2: 名前に拡張子を含む
  実フォルダ → §3.4・§1.418、P3 2 件)、第 2 回で受入可。本体 lib 全件 (`test-lib-full.ps1`、6e1cee860 時点) は
  14,168 件成功・失敗 0。実アプリ: ClaudeCode が隔離 data-dir (`target\bookrating-smoke\data`) で dev-runtime build を
  起動し、data-dir の keymap.ini (新操作 5 つに Ctrl+F3/F5/F6/F7/F8) が取り込まれて settings.db の keymap overrides に
  保存されたこと、生成された keymap.ini.default に新 8 操作が載ることを、ファイルと DB を読んで確認した。
  画面操作による確認 (2026-10-23、利用者の許可後に ClaudeCode が同じ隔離 data-dir で実施。結果は画面と rating.db の行で確認):
  一覧で bookA.zip セル選択 + Ctrl+F3 → ZIP 本体の行が★3 (kind ZipFile)。本のページ一覧で 1 ページ目選択 + Ctrl+F5 →
  `bookA.zip::01.jpg` の行が★5 (本体は不変)。2 ページ目の閲覧中に Ctrl+F8 → 本体★3→★4、表示中の 2 ページ目には行が
  できない。続けて Ctrl+F5 でトースト「[本★★★★★]」、Ctrl+Z 1 回で★4 に戻る。親一覧の bookA.zip セルのバッジも★4。
  一覧で plain フォルダのセル + Ctrl+F7 → 未評価から★2。plain (初期設定では本扱いしない) の p02.jpg 閲覧中に Ctrl+F5 →
  `plain/p02.jpg` が★5、plain フォルダは★2 のまま。別ウィンドウモード (F12) で bookA のページを別ウィンドウに開き、
  そのウィンドウで Ctrl+F6 → 本体の行が消え (解除)、表示中ページの★5・plain 側は不変。別ウィンドウでの Ctrl+Z 1 回で
  本体★4 に戻る。メインで別フォルダへ移ると別ウィンドウが閉じたため、「メインが別フォルダを表示中」の組み合わせは
  自動テストでのみ検証。native 動画 / 音声の経路は素材を用意しておらず、自動テストでのみ検証。
  以前の状態: 設計第 2.1 版 (第 2 回レビュー P2・P3 反映)。設計第 2 版 (2026-10-23、ClaudeCode)。第 1 版の独立レビュー (Codex gpt-6.1-sol / xhigh、
  SID 01a166e6-c2f3-7ff0-b8cf-e4a4ce5721a1) は条件付き受入 (P1 なし・P2 1 件・P3 1 件)。第 2 版で P2
  (新経路から `is_dir()` を除く) と P3 (入れ子区分を `at_root()` に) を反映。レビュー担当は本変更が
  症状パッチではなく対象解決の所有境界を明示する構造的変更であり、detached 述語・viewport 経路を変えない
  ことに同意した。
- 起点: 5ch >>603。利用者決定 2026-10-23 (backlog §1.417、統合時に削除)。
- 対象ブランチ / 作業ツリー: `book-preferred-rating-1417` / `G:\home\mimageviewer-bookrating`。
- 未リリース判定: 新しい操作と ini 名を追加するだけで、既存の永続データ (rating.db・keymap.ini の既存行) の
  形式・意味は変えない。マイグレーションは不要。

## 1. 何を作るか

`[Rating]` に「本を優先」の系統を 8 操作追加する。既定キーは全部未割り当て。

| ini 名 | 表示名 | 編集 |
| --- | --- | --- |
| `RatingBookPreferred1`〜`5` | 星Nを付ける（本を優先） | Assign(N) |
| `RatingBookPreferredClear` | レーティングを解除する（本を優先） | Assign(0) |
| `RatingBookPreferredStepUp` | 評価を1段階上げる（本を優先） | Step(Up) |
| `RatingBookPreferredStepDown` | 評価を1段階下げる（本を優先） | Step(Down) |

表示名は backlog の名称例「評価を1段階上げる（本を優先）」と既存の「（アイテム）」「（コンテナ）」の型に揃える。

対象の規則 (backlog の正本を実装用に言い換えたもの):

| 入力を受けた場面 | 対象 |
| --- | --- |
| 一覧 (通常フォルダ・本のページ一覧・抽出結果を問わない) | 既存のアイテム用と同じ。選択 / チェック項目。ZIP/PDF/フォルダのセルはその項目自体 |
| 画像閲覧・native 動画/音声で、採用済み `ProjectionRecipe` が `Book(scope)` | 開いている本全体 (§3 の本キー) |
| 画像閲覧・native 動画/音声で、`OrdinaryList` / `ExtractedResult(_)` | 表示中のアイテム (既存のアイテム用と同じ) |

一覧では「本を優先」= アイテム用と完全に同じ処理とする。一覧で本を評価したいときは、本のセル
(ZIP/PDF/フォルダ) を選べばよく、本のページ一覧ではページを評価する (backlog の規則どおり)。

## 2. 状態の組み合わせを減らす検討 (CLAUDE.md 2026-09-28 指示)

- 新しい状態は持たない。入力 1 回ごとに、入力を受けた viewer context の採用済み値から対象を 1 回だけ
  解決し、解決結果 (値) を既存の書き込み経路へ渡して終わる。pending / 予約 / 再試行は作らない。
- 本かどうかの判定は採用済み `ProjectionRecipe` だけを使う。全画面・見開きの状態、拡張子、
  `global_search.active`、メイン画面の現在フォルダからは推定しない。
- 本の評価キーの導出は、既存のコンテナ★ (`current_container_rating_target`) と同じ導出を共有する
  (§3)。新しいキー体系は作らない。
- 一覧は既存アイテム経路をそのまま使うので、一覧側の新しい分岐は 0。
- まれな失敗 (DB 書き込み失敗等) は既存の書き込み経路の扱い (`report_rating_write_error`) に従う。
  新しい回復処理は足さない。

## 3. 本キーの所有者 (中心の設計)

### 3.1 守る不変条件

**「本の中で『本を優先』で付けた★」と「親一覧でその本のセルに付けた★」は、rating.db の同じ行である。**
すなわち本キーは、その本のセルが親一覧で持つ `rating_path_key` と一致しなければならない。

| `BookScope` | 本キー (= 親一覧のセルのキー) | 既存の対応 |
| --- | --- | --- |
| `Archive`、`zip_nav.at_root()` | `normalize(root)`。変換書庫 (RAR/7z/LZH 等) は root = 元書庫 (`archive_source_override`) | 親一覧の `ZipFile` / `ConvertibleArchive { path }` |
| `Archive`、`!at_root()` (入れ子の本) | `book_container_key_from_segs(root, current_parent_zipdir_prefix)` (collapse で自動降下していても親セルの `ZipDir` と同じ合成キー) | 親階層の `ZipDir` セル |
| `Document(logical)` | `normalize(logical)` (PDF / EPUB 等の元ファイル) | 親一覧の `PdfFile` / 変換元のセル |
| `CompiledFolder(path)` | `normalize(path)`、kind = Folder | 本棚一覧の `Folder(path)` |
| `ImageFolder(path)` | `normalize(path)`、kind = Folder | 親一覧の `Folder(path)` |

注: `ZipNavState::new()` は初期ルートも collapse するので、`wrapper/01.jpg` だけの ZIP は `at_root()==true` かつ
recipe の prefix が非空になる (設計レビュー P3)。区分は prefix の空/非空ではなく既存導出と同じ `at_root()` で行う。
ZIP 内の PDF/EPUB を本として開く経路は現行コードに無い (書庫列挙が閲覧行を作らない) ので、対象外。

上の対応は「`BookScope` の値をそのままキーにしてよい」という意味ではない。キーは既存のコンテナ★と同じ
**context の物理状態** (`zip_nav`・`archive_source_override`・`current_folder`) から導出し、`BookScope` とは
**照合**に使う (§3.3)。これで、入れ子 ZIP の collapse・変換書庫の元書庫・EPUB の論理元など、既存の
コンテナ★が既に正しく扱っている規則をそのまま再利用し、`BookScope` 側に第二の導出を作らない。

### 3.2 型と関数

- `ContainerRatingTarget { key: String, source: PathBuf, meta: RatingMeta }` を導入する
  (今のタプル `(String, PathBuf, RatingMeta)` を名前付きにする)。
- 既存 `current_container_rating_target()` の本体のうち、検索中・合成パスの拒否**以外**の
  **キーと source の導出**を、I/O なしの `physical_container_rating_key(&self) -> Option<PhysicalContainerKey>`
  (`key`・`source`・入れ子なら `dir_prefix`) に切り出す。`current_container_rating_target()` は従来どおり
  `global_search.active` と合成パスで拒否してからこれを呼び、metadata は従来どおり
  `archive_rating_kind_for_path` (内部で `is_dir()` を呼ぶ既存処理) で作る。Shift+F・アドレスバー★・
  メタデータパネル・リングの挙動は不変。
- 新操作の metadata は**採用済み `BookScope` の種別から**作り、ファイル照会をしない (設計レビュー P2):
  `ImageFolder` / `CompiledFolder` → `Folder`、`Document` → `PdfFile`、`Archive` で `at_root()` → 拡張子で
  `ConvertibleArchive` (+ `archive_format`) か `ZipFile`、`Archive` で入れ子 → `ZipDir` (+ `dir_prefix` /
  `zipdir_is_archive`、既存と同じ値)。拡張子判定は既存の `archive_format_for_rating_path` /
  `is_zip_extension` を使う (I/O なし)。
- `write_container_rating_for_target_with_publication` の `showing_this_container` は、今
  `current_container_rating_target()` (metadata 生成で `is_dir()` を呼ぶ) を呼んでいる。これを検索中・合成パスの
  拒否 + `physical_container_rating_key()` の**キー比較だけ**にする (判定結果は同じで、ファイル照会が消える)。
- 同様に、既存 `current_container_rating_key_and_source()` (`sync_current_context_rating_session_writes()` と
  保存後の公開処理から呼ばれ、今は metadata 生成経由で `is_dir()` に到達する) も、従来の拒否条件
  (検索中・合成パス) を保持したまま `physical_container_rating_key()` へ委譲し、metadata を作らない
  (設計レビュー第 2 回 P2)。
- 新しい所有者 `book_preferred_rating` (新規モジュール `src/app/book_preferred_rating.rs`):

```rust
pub(crate) enum BookPreferredRatingTarget {
    /// 開いている本全体。入力時に確定した値。
    Book(ContainerRatingTarget),
    /// 本扱いではない閲覧。呼び出し側の既存アイテム経路へ。
    ViewerItem,
    /// 採用済み recipe は本だが、context の物理状態と照合できない。書き込まない。
    Unavailable(BookPreferredUnavailable),
}

impl App {
    /// 閲覧中 (画像 / native 動画・音声) の入力に対して、入力を受けた viewer context の
    /// 採用済み recipe と物理状態だけから 1 回解決する。I/O なし。
    pub(crate) fn resolve_book_preferred_viewer_target(&self) -> BookPreferredRatingTarget;
}
```

`resolve_book_preferred_viewer_target` は I/O をしない純粋な読み取りで、判定は次だけ:

1. `self.projection_recipe` が `Book(scope)` でなければ `ViewerItem`。
2. `physical_container_rating_key()` を求め、§3.3 で `scope` と照合する。一致すれば scope 由来の metadata を
   付けて `Book(target)`、求められない / 一致しなければ `Unavailable(理由)`。

`global_search.active` は見ない。検索結果から物理の本を開いた状態 (recipe = `Book`) でも、
その本の物理状態から本キーが決まる。検索結果一覧そのもの・検索結果から開いた画像
(recipe = `ExtractedResult`) は `ViewerItem` になるので、検索前のフォルダへは届かない。

### 3.3 照合 (BookScope と物理状態)

| scope | 一致の条件 |
| --- | --- |
| `Archive { logical_root, prefix }` | `zip_nav` があり、`zip_rating_root_path(...)` が `logical_root` と同じパス、かつ `nav.current() == prefix` |
| `Document(logical)` | `zip_nav` が無く、物理導出の `source` が `logical` と同じパスで、かつ採用済み `items` に `GridItem::PdfPage { pdf_path, .. }` で `path_eq(pdf_path, logical)` の行が 1 つ以上ある |
| `ImageFolder(p)` / `CompiledFolder(p)` | `zip_nav` が無く、`current_folder` が `p` と同じパスで合成パスでない |

パス比較は既存の I/O なし比較 (`folder_tree::path_eq` 相当) を使う。不一致は採用の途中 (recipe と物理状態の
世代のずれ) や §3.4 の共有分類不具合で起こり得る。そのときは書かずに `Unavailable` とする (§5)。

文書の照合は採用済みページ行を使う。既存の `pdf_enumerate_pending` / `pdf_placeholder_count` は列挙完了後に消え、
`display_epub_source` は EPUB のリース専用で PDF には無いので、PDF / EPUB 共通の採用済み文書状態にはできない。
文書行を作る `materialize_cached_document_rows` / `materialize_document_rows` はどちらも `document_page_item` を通り、
EPUB でも変換先 PDF ではなく元の EPUB パスを `pdf_path` に保持する。状態の追加・ファイル照会はしない。

### 3.4 名前に拡張子を含む実フォルダ (コードレビュー第 1 回 P2、§1.418)

`ProjectionRecipe::for_physical()` と `physical_page_order_locked()` は、ソースが実フォルダでも名前の拡張子で
`Archive` / `Document` に分類する (例: 実フォルダ `book.zip` / `book.pdf`)。これは §1.417 より前からある共有の
分類境界の不具合で、見開きの既定・ページ順の固定・Remote の判定にも及ぶため、根本の修正は §1.418 として分ける。
§1.417 では誤った書き込みを作らないことを保証する: `Archive` は `zip_nav` の有無で、`Document` は文書ローダーが
作った状態との照合で、実フォルダを本として扱わず `Unavailable` (書かない・通知) にする。§1.418 で分類が直れば、
これらのフォルダは `OrdinaryList` / `ImageFolder` になり、resolver の変更なしに正しい対象へ解決される。

## 4. 入力経路

すべて「入力を受けた viewer context が投影 (mount) されている状態で、その入力の処理中に 1 回解決」する。
F12 / 複数ウィンドウは、既存の画像閲覧・native 動画のキー処理がその窓の context を投影した状態で走って
いるので、新しい窓判定・述語・状態は足さない。

### 4.1 keymap

- `RatingTargetFamily { Item, Container, BookPreferred }` を導入し、`consume_rating_action(ctx, container: bool)`
  と `RatingKey { container: bool, .. }` を `family` に置き換える。表示用の既存 helper
  (`rating_key_action` / `first_rating_chord_label` / `rating_chord_summary_label`) は Item / Container だけを
  対象にしたまま変えない (既存の表示に新系統を混ぜない)。
- `ini_name()` / `context()` = Rating / `trigger()` = Press / `default_chords()` = 空 / `ALL_ACTIONS` (既存
  Rating 群の直後) / 表示名 / `docs/keymap.ini.default` (`= none`) を揃える。重複割り当ての検査は既存規則のまま。
- native 動画の `native_video_rating_action` は Container → Item → BookPreferred の順に照合する (既存 2 系統の
  優先を変えない)。

### 4.2 一覧 (`App` のグリッドキー処理、src/app.rs のレーティング節)

`BookPreferred` の編集は `apply_rating_edit_to_selection(edit)` を呼ぶ (アイテム用と同じ)。既存の
「チェック済みは一括・1 回の Undo・未評価は★3 基準・上下限で変更なし」をそのまま使う。

### 4.3 画像閲覧 (src/ui_fullscreen.rs のレーティング節)

既存の `container_rating_key` / `rating_key` の消費と同じ位置で `BookPreferred` を消費し、
`resolve_book_preferred_viewer_target()` を**その場で 1 回**呼ぶ:

- `Book(target)` → `apply_rating_edit_to_container_target(&target, edit, ContainerRatingToast::Book)`。
- `ViewerItem` → 既存アイテム用と同じ `rating_idx` の解決 (`initial_page_key` によるページ特定) で
  `apply_rating_edit_to_fullscreen_item(rating_idx, edit)`。既存アイテム用とこの経路で同じ解決を使うよう、
  `rating_idx` の解決を局所 helper にまとめる (コピーしない)。
- `Unavailable` → §5。

キー処理の後段にある既存の「公開でページが移動・並べ替え・一覧へ戻った場合の Rebased / Closed」処理は
そのまま働く。本キーは値で確定しているので、評価による絞り込み離脱・並べ替え後に別の対象へ適用されることはない。

### 4.4 native 動画 / 音声 (src/app/native_video.rs `dispatch_native_video_key_event`)

`RatingKey.family == BookPreferred` のとき同じ resolver を呼び、`Book` → コンテナ目標の書き込み、
`ViewerItem` → `apply_rating_edit_to_fullscreen_item(fs_idx, edit)`、`Unavailable` → §5。
`NativeVideoFixedKeyAction` に `RatingBookPreferred { applied_to: Book | Item | Unavailable, edit }` 相当を足す
(既存 4 variant は変えない)。通常、動画・音声を含む一覧は `OrdinaryList` なので `ViewerItem` になる。

### 4.5 書き込み

`apply_rating_edit_to_container_target(&mut self, target: &ContainerRatingTarget, edit, toast) -> bool` を新設し、
既存 `apply_rating_edit_to_current_container` はこれを使う形に書き換える (挙動不変):

1. `sync_current_context_rating_session_writes()`
2. `Step` の変更前値は、既存 `write_container_rating_for_target_with_publication` と同じ規則
   (この context がその container を表示中なら `current_folder_rating_cache`、それ以外は `rating_db.get(key)`)。
   規則は 1 か所の helper にまとめ、writer と共有する。`step_rating` が `None` なら既存の上下限トーストで終了。
3. `write_container_rating_for_target(&key, &source, &meta, stars, true)` — 保存・Undo 捕捉・表示キャッシュ
   (その container を見ている投影だけ)・公開・smart folder 更新は既存どおり。
4. トースト: Container は既存の `[フォルダ★…]` / `[フォルダ★解除]` のまま。Book は `[本★…]` / `[本★解除]`。

`write_container_rating_for_target_with_publication` 内の `showing_this_container` 判定は今
`current_container_rating_target()` (検索中は None) を使っている。検索から開いた本で `Book` 目標を書くと
showing=false となり、表示キャッシュ (`current_folder_rating_cache`) を更新しないが、検索中は
アドレスバー★が 0 固定 (既存仕様) なので表示の食い違いは生じない。この判定は変更しない。

## 5. 本だが照合できないとき

書き込まず、トースト `[この本は評価できません]` とログ `[rating] book_preferred unavailable reason=<typed>` を出す。
アイテムへの代替適用はしない (利用者の意図と違う対象へ黙って書かない)。理由は typed enum
(`NoPhysicalTarget` / `ArchiveRootMismatch` / `ArchiveLevelMismatch` / `DocumentMismatch` / `FolderMismatch`)。

## 6. やらないこと

- リング (ゲームパッド / 長押し) の評価ピッカー、Remote、メニュー項目への新系統の追加 (backlog はキー操作のみ)。
- 既存アイテム用 / コンテナ用の意味・既定キー・表示ラベルの変更。Shift+F の検索中拒否の変更。
- 本判定のための走査・I/O の追加。

## 7. テスト

- keymap: 8 操作の catalog (ini 名・context・trigger・既定キーなし・`ALL_ACTIONS` 内)、keymap.ini への保存と
  再読込、既存 Rating 割り当て (F1〜F6 / Shift+F1〜F6 / Step の none) の不変、`docs/keymap.ini.default` との同期
  (既存の同期テストがあれば追随)。`consume_rating_action` と `native_video_rating_action` の family 判定。
- resolver (純粋): `OrdinaryList` / 各 `ExtractedResult` → `ViewerItem`、ZIP ルート、入れ子 ZIP (collapse で
  自動降下した状態で親 `ZipDir` セルと同じキー)、変換書庫 (元書庫のキー)、PDF、EPUB、`ImageFolder`、
  `CompiledFolder`、検索から開いた物理の本 (検索前フォルダではなく本のキー)、recipe と物理状態の不一致 → `Unavailable`。
  各ケースで「本キー == 親一覧でそのセルの `rating_path_key`」を直接比較する。ZIP は「初期ルートで collapse
  した本 (`at_root()` かつ prefix 非空)」と「入れ子に入った後で collapse した本」を別のテストにする。
  Ctrl+G は「検索を開いた直後 (旧の本の items と recipe がまだ採用済み)」と「検索結果を採用した後
  (`ExtractedResult`、検索前フォルダへは書かない)」と「検索結果から物理の本を開いた後 (本のキー)」を区別する。
  resolver がファイル照会をしないこと (存在しないパスでも同じ結果になること) も確かめる。
  文書は一致する採用済み `PdfPage` 行を必須とし、行なし・画像行・文書セル・別文書のページ行を拒否する。
- 実採用経路: tempdir の実フォルダ `book.zip` / `book.pdf` (画像入り) の親一覧を採用し、実際の `Folder` セルの
  キーを取得してから `load_folder` で開き、
  閲覧中の本優先入力がフォルダ / ページのどちらにも書かず、Undo も積まない。本物の ZIP は通常の
  worker / 採用経路から開き、同じ入力で親一覧の本セルと同じキーへ書く。
- handler-level: 一覧 (カーソル単体・複数チェック 1 回 Undo・本のページ一覧) は選択項目に書く。画像閲覧の
  本では本キーへ書き、ページの★は変わらない。非本では 3 項目の選択位置と閲覧位置を別にして、閲覧項目だけへ書く。
  Step は未評価→★4 / ★5 で変化なし。評価フィルタで現在の本や項目が条件から外れても 1 回だけ適用される。
  項目は 3 項目の中央を閲覧中に外し、次項目へ移り、Undo 1 回で値と元の閲覧位置が戻る。
- 複数 context: F12 で本 A を開き、メインは別フォルダ B を表示している状態で、F12 側の入力は A のキーだけに
  書き、メイン context の items・rating_cache・current_folder_rating_cache のうち A と無関係なキーを変えない
  (メインの一覧に A のセルがあれば、既存の共有 ledger による次回 mount 時の★更新は正しい既存動作)。
  メイン側の入力は B 側の規則で解決する。
- native 動画: 非本の動画では表示中アイテムへ。`NativeVideoFixedKeyAction` の結果を確認する。

## 8. 文書

keymap-spec.md・key-customization-impl-plan.md・keymap.ini.default・tag-storage-and-xmp.md (本の★の保存先が
コンテナ★と同じ行であること)・rating-filter-fullscreen-plan.md (閲覧中の対象規則)・マニュアル
(grid.html のレーティング節・shortcuts.html・tut-rating.html・video.html / music.html の該当箇所)・
detached-rework-plan.md §11 (F12 経路に触れた範囲と判断)。
