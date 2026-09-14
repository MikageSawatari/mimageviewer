# 一覧のファイルサイズ順（§2.24）

最終更新: 2026-09-14

## 1. 目的と状態

通常一覧の `SortOrder` に次の2項目を追加する。

- サイズ順（小さい順）
- サイズ順（大きい順）

この文書は実装前調査、合意済み設計、実装・検証記録の正本である。設計は独立検収で
blocking 0となり、製品実装とfocused回帰まで完了している。最終full/static gateと確認build、
GUIでの実表示確認は後段の検証記録へ追記する。

対象は通常フォルダと、同じ一覧並び順を使う検索・スマートフォルダ・サブフォルダ展開・
レーティング・ブックマーク・ファイル名スタック・Remote一覧である。詳細一覧の列見出し
`DetailsSortKey::Size` は既に独立実装されているため挙動を変えない。

## 2. 現状調査

### 2.1 設定と入口

`src/settings.rs` の `SortOrder` は FileName / Numeric / DateAsc / DateDesc の4値で、
`settings.sort_order` と `FavoriteViewState.sort_order` が同じ型を保存する。メインメニュー、
ツールバー、ツールバー設定、ゲームパッド、Remoteの選択肢は `SortOrder::all()` を正本にする。
Remoteの値は serde のstable stringを使い、別のIPC enumはない。

一方、`settings.folder_thumb_sort` も同じ型を使うが、これはフォルダ代表サムネイルの探索順で
あり一覧順とは独立している。環境設定の代表画像候補が現在 `SortOrder::all()` を列挙している
ため、一覧候補を2値増やすだけでは代表画像設定へも誤ってサイズ順が現れる。

### 2.2 メタデータと0バイト

現在の共通表示メタデータは `Option<(mtime, file_size)>` で、キャッシュ鮮度・詳細表示・
動画準備にも使う。通常フォルダのscan失敗と検索placeholderには `Some((mtime, 0))` または
`Some((0, 0))` があり、実在する0バイトファイルも `size == 0` になる。この値だけでは
UnknownとKnown(0)を区別できない。タプル全体を `None` にするだけでは、sizeだけ不明でも
既知のmtimeを捨てる一般契約になる。

サイズ順の比較境界には次のtyped metadataを使う。

```rust
ListingSortMetadata {
    mtime: i64,
    file_size: Option<i64>, // Some(0) は実在する0バイト、Noneは不明
}
```

`SortOrder::compare_listing_keys` が名前keyとこの値を受ける。SizeAsc/SizeDescはKnownを値で
比較し、Unknownは両方向とも末尾、同値は常にWindows互換のファイル名昇順にする。降順で
比較全体をreverseせず、Unknown-lastを明示する。

通常フォルダscanはmetadata成功/失敗から `ListingSortMetadata.file_size` をKnown/Unknownとして
別成果へ保持する。materialize中は既存の表示用 `image_metas` と別のaligned
`ListingSortMetadata` を
itemsと一緒にfilter・4行配置の完了まで運び、順序確定後だけ破棄する。したがってsizeがUnknown
という理由で既知mtimeを消さない。現行の通常folder scanでは1回の `DirEntry::metadata()` から
mtime/sizeを同時に得る。表示用値は現在の契約をexact維持し、Folder/物理fileともscan失敗時は
`Some((0, 0))`、成功したFolderも `Some((mtime, 0))` のままにする。将来または別producerがmtimeだけを
持つ場合もtyped sort metadataのmtimeはそのまま保持できる。Folderや仮想項目はsize Unknown、実ファイル
（Image/Video/Audio/ZipFile/PdfFile/ConvertibleArchive）は既存metadata成功時だけKnownとする。
この判定は `GridItem` の共通helperに置き、各UIにkind表を複製しない。mtimeとキャッシュ用sizeの
既存意味は変えない。

### 2.3 一覧consumer

| 経路 | 現在の値 | サイズ順で使う値 |
|---|---|---|
| 通常フォルダ / Remote folder | `DirEntry::metadata` をscan済み。Remoteも `materialize_local_folder_listing` を共有 | scan成功の物理file size。FolderはUnknown |
| Ctrl+G flat/drill | Tantivyに `file_size` が既にSTORED。workerは現在mtimeだけ読む | post-filter通過docからmtimeとsizeを同時取得。index候補はmetadata失敗時に登録されないためstored 0は実0バイト。FS drillの直下実hitだけKnown、合成子FolderはUnknown。ZIP drill/pageは固定順。schema/version bumpやUI statなし |
| スマートフォルダ | worker scanの `SmartFolderEntry.file_size`。失敗を0へ畳んでいる | entryで `Option<i64>` を保持。既存size filterは現在 `size <= 0`（失敗と実0バイト）を除外しており、変更後もUnknownとKnown(0)をともに除外するのでfacet集合は不変 |
| サブフォルダ展開 | 物理fileはmetadata失敗時に候補から除外。合成本Folderはsize 0 | 物理fileはKnown、合成本FolderはUnknown |
| ファイル名スタック | memberは既存 `image_metas` 由来 | 通常scanの別aligned sort metadataからmemberを作り、missingはUnknown。集約Stackセル自身は常にUnknown |
| レーティング / ブックマーク | workerの `image_meta` と復元済み `GridItem` | 実fileだけKnown。ZIP/PDF page・Folder等はUnknown |
| Remote smart folder | 本体と同じsmart snapshotをworkerでmaterialize | 本体と同じ比較後の順序を返す。追加I/Oなし |
| 詳細列のSize | `DetailsSortKey::Size` 独自 | 変更しない |
| 閲覧履歴・タグ・本棚等の固定一覧 | view固有順でlock | 変更しない |

`docs/virtual-folders.md` のFolder / Archive / Image / VideoAudioの4行割当を先に適用し、
サイズ比較は同じ割当行の中だけで行う。Unknownを別カテゴリへ移動しない。Flatと
FolderGroupedを持つsmart/subfolderは既存group keyを保ち、その内部比較だけを差し替える。

### 2.4 本と代表サムネイル

次はサイズ順の対象外とする。

- 画像だけの通常フォルダを本扱いする場合は `BOOK_READING_PAGE_ORDER = FileName`。
- compiled bookはNumeric、ZIPはFileName、PDFは列挙順という既存の固定ページ順を保つ。
- `page_order_locked_for_current_view` とRemoteのlock表示を変えない。
- フォルダ代表サムネイルは `folder_thumb_sort_options()`（既存4値だけ）を新たな正本にし、
  環境設定とworker/cache keyの両方でSizeを選ばせない。手編集・将来の不正値としてSizeが
  読まれた場合はsanitizeでFileNameへ戻す。

これにより `SortOrder::all()` を一覧6値へ拡張しても、本内部や代表画像候補へ波及しない。

## 3. 実装案

### 3.1 共通比較

`src/settings.rs` に SizeAsc / SizeDescとラベル・短縮名・説明を追加する。`all()` は一覧6値を
返し、`folder_thumb_options()` は既存4値を返す。`ListingSortMetadata` と
`compare_listing_keys` を同じownerに置く。既存 `compare_name_keys` は名前・日付・固定ページ
順の互換APIとして残し、Size variantを渡した場合は名前tiebreak相当になるよう全matchを保つ。
一覧consumerは必ずtyped APIを使う直接回帰を置き、sizeを欠く旧APIへ依存しない。旧APIへ
SizeAsc/SizeDescが来た場合はdebug/test buildでfail-fastし、releaseだけ決定的なファイル名fallbackを
持つ。これにより新規一覧callerの移行漏れをテストで隠さない。

`src/grid_item.rs` の4行materializerはitem kindとaligned metadataからtyped sort metadataを
一度作り、各行内だけ比較する。新しいtyped overloadはitems / image_metas /
listing_sort_metasの3本を同じ入替えで運び、長さ不一致をassertする。既存2本のwrapperはsizeを
必要としない固定順・互換testだけに残す。`sort_folder_block` もtyped metadata版を正本にする。

### 3.2 worker成果の伝搬

- `src/app/folder_scan.rs`: physical mediaのsizeを `Option<i64>` で保持する。duplicate filterは
  path/kindだけを見る既存契約を維持する。表示メタの `Some((mtime, size))` / `Some((0, 0))`
  は現状exactで維持し、sort metadataだけを別vectorで4行配置完了まで保持する。
- `src/fts_index.rs`, `src/global_search.rs`, `src/global_search_ui.rs`: 既存STORED sizeをhitへ運び、
  flat/drillのsort rowに使う。Tantivy schema変更と再indexは不要。
- `src/app/smart_folder.rs`: scan失敗をUnknownとして保持し、size filter/cache metadataとの
  変換を明示する。本体とRemoteで同じcomparatorを使う。
- `src/app/subfolder_expansion.rs`, `src/filename_stack.rs`, `src/filename_stack_ui.rs`,
  `src/rating_view.rs`, `src/bookmark_browser.rs`: 既存group/category ownerを保ったまま、行内比較へ
  typed sizeを渡す。

UI threadで `metadata` / `canonicalize` / folder rescanは追加しない。既にscan/index/DB workerが
作ったimmutable snapshotだけを使用する。

### 3.3 旧比較APIのcaller分類

`compare_name_keys` へSizeを流して名前fallbackになる経路を放置しない。実装時に全callerを次の
3群へ固定し、一覧群はtyped comparatorへ移す。

- **一覧へ移す**: folder scan / grid item 4行 / global search / smart folder /
  subfolder expansion / filename stack / rating / bookmark、および同じpure helperを使うRemote。
- **Sizeを選択不能にする**: folder representativeの `thumb_loader`（旧4候補だけ）。
- **固定orderのまま**: `zip_tree` とZIP/PDF/book materialize（FileNameまたはNumeric定数）、
  FolderTreeSortOrderを使うfolder tree/pane、benchmark/test専用caller。

`rg compare_name_keys` のcaller一覧を最終検証台帳へ残し、新しい一覧consumerが旧APIへ増えていない
ことをstatic checkで確認する。folder representativeの読込sanitize+4候補、ZIP/bookの呼出値が
固定定数であることも直接回帰で固定する。

### 3.4 Ctrl+Gとファイル名スタックの投影

Ctrl+G flatとFS drillでは、Tantivyの実hitだけstored sizeをKnownとして比較する。drill用に合成した
子FolderはUnknownとし、ZIP内ページへ一覧Sizeを適用しない。ZIP drillは既存のcontainer/page固定順を
維持する。

ファイル名スタックは2段を分ける。個々のphysical memberはscan時のtyped sizeを保持し、展開した
member内部だけSizeで並ぶ。複数memberを畳んだ `GridItem::Stack` は代表memberのsizeをgroup sizeへ
昇格せずUnknownとする。集約gridのgroup順はgroup名の既存決定順を保ち、Sizeで代表memberを選び
直さない。スタック化されない単独physical mediaセルだけはKnown sizeで通常どおり並ぶ。

### 3.5 保存と互換

serdeへenum variantを追加するため、旧settings/Favorite JSONはそのまま読める。新しい値を
旧版が読めない通常のdowngrade境界は、現Settingsのbackup/version運用に従う。

新規installの `toolbar_sort_items` は6値を含む。旧版には無いone-time migration markerを保存し、
marker未設定かつ既定4値がexact canonical orderの場合だけSizeAsc/SizeDescを既定位置へ補完する。
markerを同じbootstrap saveで立てるため、移行後に利用者がサイズ2候補だけを非表示にして
canonical4へ戻しても次回loadで復活しない。空Vecや順序変更・一部非表示など利用者custom値は
変更しない。重複/unknown処理の既存規約を維持する。Favorite別
`sort_order` は同じ型のため追加fieldやDB schema変更なしでroundtripする。

Remoteはserde stable stringと既存options builderを使い、core/Web UIの同梱sourceから6候補を
返す。通常folderとsmart folderの実順序を本体と一致させ、本/固定collectionのlockは維持する。

## 4. 変更予定path

中心:

- `src/settings.rs`
- `src/grid_item.rs`
- `src/app/folder_scan.rs`
- `src/app/smart_folder.rs`
- `src/app/subfolder_expansion.rs`
- `src/global_search.rs`
- `src/global_search_ui.rs`
- `src/fts_index.rs`
- `src/filename_stack.rs`
- `src/filename_stack_ui.rs`
- `src/rating_view.rs`
- `src/bookmark_browser.rs`
- `src/ui_dialogs/preferences/pages.rs`
- `src/remote_ipc/mod.rs` と必要な既存Remote回帰

文書:

- 本文書
- `docs/spec.md`
- `docs/virtual-folders.md`
- `docs/subfolder-expansion-view-plan.md`
- `docs/search-container-item-redesign.md`
- `htdocs/mimageviewer/manual/grid.html`
- `htdocs/mimageviewer/manual/settings.html`
- `htdocs/mimageviewer/manual/tut-grid-power.html`（既存の一覧sort説明がある場合）

機械的test constructor更新は該当module内に限定する。親ownerのbacklog/開発台帳/READMEは触らない。

## 5. 回帰と受入条件

### 5.1 純比較とカテゴリ

- SizeAsc: Known(0), Known(1), Known(10), Unknown。SizeDesc: Known(10), Known(1), Known(0), Unknown。
- 同sizeとUnknown同士は名前昇順。入力列挙順に依存しない。
- 4行カテゴリ割当は従来exactで、Unknownは各行の末尾。他行へ移らない。
- Folder/仮想page/StackはUnknown、実0バイトfileはKnown(0)。metadata失敗はUnknown。

### 5.2 各consumer

- 通常folderとRemote folderが同じzero/known/unknown順を返す。
- Ctrl+Gはstored sizeで並び、UI filesystem I/Oが増えない。0バイトもKnown。
- smart folder本体/Remote、subfolder Flat/FolderGrouped、filename stack、rating、bookmarkで
  category/groupを保ちsize順と名前tieが一致する。
- metadata failureは表示メタを変えずUnknown末尾。smart size facetの既存集合は変えない。
- Ctrl+G FS drillの実hitはKnown、合成FolderはUnknown、ZIP drill/page順は固定。
- collapsed StackはUnknownでgroup順/代表を変えず、member内部と非stack単独mediaだけKnownを使う。
- 本扱い画像folder/compiled book/ZIP/PDF/閲覧履歴の内部順とsort lockがexact不変。
- folder representativeの候補・選定/cache keyは既存4値だけで、Sizeを選べない。

### 5.3 保存/UI/Remote

- SettingsとFavorite view stateが2値をroundtripし、Favorite切替で共通設定と独立する。
- 旧JSON欠落はFileName。旧canonical toolbar4値はone-time marker未設定時だけ6値へ補完し、
  移行後にサイズ2候補だけを外したcanonical4とcustom/emptyは再loadでも不変。
- メニュー、ツールバー設定、gamepad、Remote optionsに同じ6値とラベルが出る。
- Remote SetSortOrderの2 stable valueがparse/persistでき、book lock中は変更不可の既存契約を保つ。
- サイズ順を選んでも詳細Size列の独立sort/reset契約を変えない。

## 6. 検証計画

まず比較・normal materializer・各virtual consumer・settings/Favorite・Remoteのfocused testを実行する。
共有enum、scan metadata、検索hit、Remoteを横断するため、独立review後に
`scripts/test-full.ps1`、`cargo check -p mimageviewer --bin mimageviewer-core`、
`cargo fmt --check`、UI文字glyph check、viewer-context audit、diff checkを通す。

自動検証がgreenでresident processがないことを確認して
`scripts/build-dev.ps1 -PreserveRuntime` を実行する。アプリは起動しない。GUIでの実表示確認は
利用者帰宅後の保留事項として、自動検証・確認buildと区別して記録する。

## 7. 実装checkpoint（2026-09-14）

- `SortOrder` を一覧用6値へ拡張し、代表サムネイル用4値を別catalogに固定した。
- scan/index/DB worker由来のsize availabilityを `ListingSortMetadata` として表示metadataと
  分離し、通常一覧・検索・Smart・サブフォルダー展開・Stack・評価・ブックマーク・Remoteへ
  同じ比較契約を接続した。
- 物理0-byteはKnown(0)、取得失敗・Folder・仮想page・collapsed StackはUnknownのまま、
  Unknown-lastと名前tieを昇順・降順の両方で固定した。
- 本内部、ZIP/PDF page、folder代表サムネイル、詳細列Size sort、Smart size facetの既存契約は
  変更していない。
- focused回帰はsize comparator 9件、grid item 23件、folder scan 14件、Smart 34件
  （ignored 1）、subfolder 33件、global search worker 18件、global search UI 40件、Stack 25件、
  rating 8件、bookmark 15件、および保存・Remote・本固定順の各直接回帰を通過した。
- `cargo check -p mimageviewer --bin mimageviewer-core` と `cargo fmt --all -- --check` は通過した。

GUIでの一覧メニュー・toolbar/gamepad表示確認は利用者帰宅後に行う。自動検証と確認buildの
完了をGUI確認済みとは扱わない。

## 8. 最終検証（2026-09-14）

独立reviewで、表示metadataを変えない別aligned sort metadata、Unknown/zeroの区別、
本・ZIP/PDF・folder代表の固定順、全consumerのtyped comparator接続を確認した。reviewで見つかった
次の境界も最終sourceへ反映した。

- toolbar既定4値から6値への補完は保存markerによるone-time migrationとし、現行schemaで利用者が
  Size 2候補だけを外したcanonical4を再補完しない。現行 `Settings::default` は6候補かつ
  marker=true、旧serde field欠落だけがmarker=falseになる。
- Stack materializeのitems/display metadata/sort metadataは3列完全一致をassertし、黙ったtruncateや
  Unknown化をしない。
- Ctrl+Gの合成FolderはSize選択時だけtyped Unknown comparatorへ通し、Windows filename tieを使う。
  非Sizeは従来の `PathBuf` 順を維持する。

focused回帰は§7の群に加え、上記3境界、Settings DB roundtrip 3件、backup snapshot 1件を通過した。
最初のfull gateはmain library 8430 passed / 4 failed / 45 ignoredで、4件はいずれも
current `Settings::default` のmigration markerがfalseだったため、DBからload-time migrationを
通した値のtrueと一致しない同じ原因だった。他targetの実行は継続したが、このrunは不採用とした。
current defaultをtrueへ補正し、旧serde欠落のfalseを維持した後、該当5 focused回帰を通してから
full gateを一度だけ再実行した。

- `scripts/test-full.ps1`: exit 0、main 8434 passed / 0 failed / 45 ignored、UI snapshot
  50 passed、vendor egui 25 / egui-wgpu 9 / eframe 15 passed、末尾 `[test-full] PASS`。
- `cargo check -p mimageviewer --bin mimageviewer-core`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `python scripts/check_ui_glyphs.py`: exit 0、dangerous glyph 0。
- `cargo run -p viewer_context_audit --quiet`: exit 0、finding 0。
- `git diff --check`: exit 0。
- resident process 0を確認後、`scripts/build-dev.ps1 -PreserveRuntime`: exit 0。
  `target/dev-runtime/mimageviewer-core.exe` SHA-256
  `1C1FF62B8ADFBA0FCE645B74F25CFF5FFF0B40AB2EB9CEB24C01E974AE2513A2`、
  `mimageviewer-remote.exe` SHA-256
  `192E2F800704A833C46831C2A42C47F24558B17A19C80F3DC23BEC21CE7D057D`。

完全ログは `target/list-size-sort-20260914/` に保存した。採用fullは
`test-full-pass.{stdout,stderr}.log` と `test-full-pass.exit.txt`、buildは
`build-dev.{stdout,stderr}.log` と `build-dev.exit.txt`。最初の不採用fullは
`test-full-final.*` として分離した。agentはアプリを起動・停止していない。
一覧のGUI実表示確認は利用者帰宅後まで未実施であり、自動/Cargo/build-backed受入と区別する。
