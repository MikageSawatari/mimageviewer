# 表示するファイル種類 — §1.345 / 履歴再入場のfacet退避 — §1.339

作成: 2026-10-07、改訂: 2026-10-08（利用者決定の確定記録）、ラインA。
状態: **仕様確定、§1.339/§1.345は未実装**。利用者決定とbf509352dの独立レビュー承認は§12に記録。
本書の新しい型・APIは未実装の設計仕様であり、現行コードに存在するという主張ではない。
実アプリは起動していない。§1.339の観測者は利用者、原因の根拠は下記のコード調査。

## 1. 決定済みの範囲と今回の境界

[次版バックログ](next-release-backlog.md)の「次の版の決定 (利用者 2026-10-07)」を正本とする。

- §1.345は既存facetの継承拡張をやめ、mIV全体の「表示するファイル種類」設定に作り直す。
  一覧生成の最初の層で除く。同名処理・AppleDouble除去と同じ生成段に置き、退避・復元を持たない。
- §1.339は既存facet退避の不具合として独立して直す。§1.345の実装・設定ONを前提にしない。
- 本書は設計のみ。適用範囲・本判定・操作導線などの利用者判断は2026-10-08に確定した（§8）。
  今回は決定の記録だけを行い、実装は追加しない。
- §1.328の残り（閲覧履歴・サブフォルダ展開のopened-row anchor/scroll未保持）は別原因。
  §1.339で選択位置やaspectの保存ownerを代替・拡張しない。今回の実装対象ではない。

関連正本: [一覧と履歴](folder-history-location-plan.md)、[仮想フォルダ](virtual-folders.md)、
[検索](search-architecture.md)、[代表画像](folder-representative-plan.md)、
[サブ展開](subfolder-expansion-view-plan.md)、[Smart Folder](section257-smart-folder-navigation.md)、
[コレクション](collection-implementation-plan.md)、[Remote](web-remote-plan.md)、
[キー](keymap-spec.md)、[永続設定](settings-sqlite-migration.md)、
[応答性](ui-responsiveness.md)、[detached憲法§2](detached-rework-plan.md#2-憲法-全ステージ共通の不変条件禁止事項--最重要)。

## 2. コードで確かめた前提

| 現行の事実 | 根拠（関数名を検索して追跡する） |
| --- | --- |
| 通常フォルダのPC/Remoteは同じ分類・一覧materializeを使う | `src/app/folder_scan.rs::scan_directory_with_settings` / `materialize_local_folder_listing_with_order`、`src/remote_ipc/container.rs::recompute_folder_listing_with_rating_read` |
| 同名除去は複数のproducerにある。一覧全部を既に通す単一関数はない | `folder_scan.rs`の各`filter_*duplicates`、`smart_folder.rs::normalize_smart_folder_candidates`の同名処理、`subfolder_expansion.rs`のmedia filter |
| 最後の`install_new_items`だけで除去すると遅い。aligned metadataや集約件数・代表画像・本判定が既に作られている | `app.rs::install_new_items_inner`、`collection_grid.rs::install_collection_grid_items_with_thumbnail_sources`、`global_search_ui.rs`のSearchContainer構築 |
| RAWも`GridItem::Image`。EPUBも`PdfFile`。種別enumだけでは要求された分類に足りない | `src/grid_item.rs::GridItem`、`src/raw_format.rs`、`folder_scan.rs::scan_directory_entries` |
| 本判定とページ数認識には共通述語・fingerprintがある | `folder_scan.rs::is_image_only_book_contents` / `image_folder_page_count` / `image_page_recognition_fingerprint` |
| Ctrl+上下は独立したDFS/stop predicateを持つ | `src/folder_tree.rs::FolderTreeOptions` / `folder_should_stop_with_options`、`docs/async-architecture.md`§3.1.5 |
| コレクションには表示行と登録entryの別identityがある。exportもprepareを再利用する | `src/collection_store/prepare.rs::prepare_collection_snapshot` / `prepare_collection_export`、`collection_grid.rs::prepare_collection_grid_install` |
| 設定転送はSettingsの全fieldを明示分類する | `src/settings_transfer.rs::preferences_policy!` / `exhaustive_policy` |
| resumeの現行検証はraw indexの範囲/ページkindだけで、ページidentityを照合しない | `app.rs::resume_page_for_container` / `is_readable_page_idx`。PNGを除外してindexを詰めると別JPEGへ誤着地し得る |
| facet scope/stackはApp-global、main grid用。context bundleにもA/B slot別stashにもない | `app.rs::rebuild_visible_indices_preserving_facet_scope` / detached分岐、`viewer_context_registry.rs::swap_viewer_context_bundle` |
| 内部書庫も実directoryも同じZipDir。is_archiveはsuffix推定で採否の根拠にできない | `zip_tree.rs::build` / `segment_is_archive`、`zip_loader.rs::enumerate_recursive` |
| collection並べ替えは全entryを保持し、DBはID subsetを拒否する。既存subset merge ownerはない | `ui_dialogs/collections.rs::open_collection_reorder` / `start_collection_reorder_save`、`collection_store/db.rs::reorder_manual` |

「同名除去と同じ層」は既存一関数への一行追加という意味ではない。
必要なのは**一つのポリシーownerをすべてのproducerから呼ぶこと**であり、
生成済みGridItemをpaint/visible_indicesでだけ隠す方法ではない。

## 3. 提案する単一ownerと分類

`src/file_type_visibility.rs`（提案）を種類の分類・採否の唯一のownerとする。
`Settings.file_type_visibility: FileTypeVisibility`からimmutableなポリシーを作り、
worker/options/requestの既存snapshotに渡す。AppとRemoteで同じ実装を使う。
一覧ごとの設定、履歴stash、favorite overlay、Remote端末別overrideは作らない。

推奨の保存形は、除外category集合と除外extension集合。空集合を既定にし、
後で対応形式が増えても初期状態で表示できるようにする。UIは「表示する」のcheckboxで見せる。
category候補は画像 / RAW / 動画 / 音声 / ZIP・CBZ / PDF / EPUB / RAR・CBR / 7z・CB7 / LZH・LHA。
拡張子はdotなしASCII小文字へ正規化し、重複を除く。別名拡張子は分類では同群、
個別除外では実際のextensionを使う。folder/driveは通行可能な構造として保持する既定（D2）。
形式一覧は既存recognizerを使い、未対応形式を本設定で新たに読めるようにはしない。

pure API案:

```rust
FileTypeVisibility::allows(&ListingCandidateType) -> bool
// Logical source identity + recognized role; no fs/DB/decode.
// PhysicalFile, ArchiveMember, DocumentPage, Directory, StructuralReference.
```

`ListingCandidateType`はGridItemを新設するための型ではなく、生成前の識別情報。
元RARをcache ZIPと分類したり、EPUBをPDFと分類したりしない。
ZIP/RARの内部画像はmember名の拡張子、PDF/EPUBページは「描画された画像」ではなく
論理元文書のPDF/EPUB categoryを使う確定仕様（Q3a、利用者決定2026-10-08）。実directoryは通路として保持するが、
内部書庫も現行ではZipDirであり、別のarchive member本体rowは存在しない。
内部書庫の選別には次の来歴を運ぶ設計が必要で、is_archive/suffixは採否に使わない。

### 3.1 内部書庫の来歴（r3追加、Q16）

`zip_loader::enumerate_recursive`の実file entryを開く境界で、
`ArchiveContainerOrigin`（未実装）を`ZipEnumeration`の列挙結果へ付ける確定仕様（Q16、2026-10-08）。
外側からの実entry locator列（各central-directory indexと実member名）、形式、
navigation prefix、各画像のcontainer ancestryを同じ列挙ownerが生成する。
画像パスをsplitしただけでは実directoryか書庫かを判別できない。
空の書庫を表示する新機能は追加せず、既存画像列に対応する境界の来歴を保存する。

`ZipTree::build`は画像とoriginを一緒に受け取り、純粋な表示projectionは
実書庫境界だけをFileTypeVisibilityへ通し、除外書庫の配下画像・代表・件数を除く。
例えば実directory `notes.zip/cover.jpg`は残し、実file `book.zip`の中の画像はZIP除外で消す。
同名directoryと実書庫が同じnavigation prefixへaliasする場合も、leafのlocator ancestryで
書庫側だけを除く。実directoryの来歴があるnodeは通路として残し、実書庫だけのnodeは
その除外と共に消す。prefix単位の一括除去はしない。
既存entry_name/永続keyは変えず、index列は列挙snapshot内の識別にだけ使う。
Remoteの逐次candidate列挙にも実entry descentの同じownerを接続する。

非ZIP書庫を展開した変換ZIPでは、元の書庫境界が実directoryへ変わるため、
変換workerから同じoriginのmanifestをcacheへ出力し、cache列挙workerが読み合わせる必要がある。
旧cacheにmanifestがなければsuffixから捏造せず、来歴が必要なときは再変換を求める
確定仕様（Q16、2026-10-08）。内部書庫の形式除外を明示的に対象外にする案は採らず、
未知来歴のまま「全対応」とは出荷しない。
来歴をpaintで再走査する案と、全ZipDirを常に残す案は、誤除外/適用漏れになるため不採用。

### 3.2 共通の投影順序

missing collection/bookmark参照はlast-known kindと論理元pathで判定し、
型を推定できない参照は保持する案。選別にstatを足さない。

生成順は、対応形式認識・OS/system/AppleDouble除去 → **本ポリシー** →
同名優先・重複除去 → 本判定/stack/集約/ソート/代表選定 → aligned install →
現在地のfacet/名前/評価絞り込み。新設定とfacetはANDであり、facetから除外項目を復活させない。
例: 動画を除外したら同名JPEGを動画の存在で消さず、ZIPを除外したら同名RARをZIPの存在で消さない（D4）。
同名判定用にdirectoryの全候補が必要でも、勝者候補集合は本ポリシー通過後にする。

## 4. 全producerの接続表

各行の入口は現行コード。接続位置とownerは提案。すべて§3の`FileTypeVisibility`へ通す。
表示rowを除く場合は同じindexでmeta・binding・stable key・thumbnail sourceも除き、
producer側で同じ入力列を保ったまま結果を組み立てる。install後のVecだけをretainしない。

| 一覧producer | 共通ownerを通す位置・注意 |
| --- | --- |
| 通常folder / 製本folder / 本棚 | `folder_scan.rs::scan_directory_entries`のrecognition後、`materialize_local_folder_listing_with_order`の同名処理前。manifestページは元ページ形式で判定し、製本データを編集しない |
| ZIP/CBZ内部 | `zip_loader.rs::enumerate_recursive`の列挙来歴を§3.1どおりZipTreeへ渡し、`materialize_level`のページ/代表/ZipDir投影前に判定。生treeを設定別に破壊しない |
| PDF/EPUB内部 | `app.rs`のPDF prepared enumerationからページitemsを作る前。logical pathの文書categoryを使う。PDF page_num、EPUB generation/leaseは維持 |
| converted archive / direct RAR | 外側tileは元形式。load_path/overrideはI/O用identity。内部は§3.1のconverter origin manifest / direct RAR列挙来歴を同じtree投影へ渡す。元形式を保持しcache ZIPと誤分類しない |
| rating list ★1〜5 | `rating_view.rs::prepare_rating_view` / `sort_and_materialize_rows`のrows→items前。source/meta/★時刻を同時に投影し、rating.db行は残す |
| 検索 Ctrl+S / Ctrl+G / Ctrl+F | `app.rs::apply_favsearch_results`、`global_search_ui.rs`のstream結果→flat/SearchContainer集約前。除外hitをhit_count・代表に含めない。Ctrl+Fは生成済み基礎一覧を対象にするので新しい別判定を足さない |
| tag view | `tag_view.rs`のDB結果→行、`app.rs::apply_tag_view_result`の集約前。タグ索引/DBは削除しない |
| collection root / physical child | `collection_grid.rs::prepare_collection_grid_install`でsource resolution後、表示binding/thumbnail source/reader order前。`collection_store/prepare.rs`のexport用full snapshotに表示除外を入れない。子はfolder/virtual共通経路 |
| collection手動並べ替え画面 | `ui_dialogs/collections.rs::open_collection_reorder`の全登録snapshotを保持し、§5.4の新しい表示projection/全ID merge adapterで同ownerを通す。DBへsubsetを送らない |
| bookmarks（全体と本内） | `bookmark_browser.rs::sort_and_materialize_rows` / `app.rs::install_bookmark_view_rows`、本内bookmark項目構築時。ZIP内画像はmember、文書ページはlogical documentで判定。bookmark ID/positionは保持 |
| reading history | `app.rs::install_reading_history_entries`から`reading_history_load_inputs`へ渡す前。元書庫・元EPUB・video/audio形式で判定し、DB履歴/進捗を消さない |
| smart folder root / scoped child | `app/smart_folder.rs`のdirectory candidate正規化前、`scan_smart_folder`と`build_remote_smart_folder_entries`で同policy。保存済みSmart ruleは次層でAND。childは通常/virtual producer |
| subfolder expansion | `subfolder_expansion.rs::scan_one_directory`の同名処理・画像本への畳み込み前。snapshot再install/stack切替も同policy。設定変更前snapshotを無条件に再利用しない |
| drive list | `app.rs::enter_drive_list`のdrive構造rowは通す。pin/代表候補を同policyで選ぶ。ドライブ配下を空かどうか調べる追加走査はしない |
| Remote listings | `remote_ipc/container.rs::folder_list` / `recompute_folder_listing_with_rating_read`はfolder共通owner、ZIP/PDF/page/bookmark一覧は各virtual owner、Smart/Rating/History/Bookmarks等の公開producerも同owner。`persistent_collections.rs::exact_prepared`等はcollection表示projectionを共有。`remote-web/http.rs::api_list`で独自filterしない |

追加監査対象: filename stackのflat/aggregate復帰、Search/Snapshotのcached restore、
notify/reload、直接path/起動復元、A/B、book-root item生成、playlist/next-item読順。
別経路で旧itemsをswapしても設定前の非表示項目が戻らないよう、
既存request/reuse-keyにpolicyの値を含め、違う場合は既存reload/prepareを使う。
一括removeによるindex詰め替えをUI threadで行わず、workerでaligned projectionを完成させる。

## 5. 派生動作と保存データ

### 5.1 代表サムネイル

自動代表は通過した候補から従来順序・depth・catalog-only archive規則で選ぶ。
RAWを除外したら自動代表のRAWも除く。pinが除外形式の場合はpin登録を消さず、
有効候補の自動代表（なければアイコン）へ表示だけ切り替える確定仕様（Q5、2026-10-08）。
動画のsidecar画像はメタデータ資源なので、画像を一覧から除外しても表示中の動画のsidecarには使い続ける。
これをfolderのpinとして使う場合は一覧代表候補の判定を行う。

既存`FolderThumbProvenance`とproofだけではpolicy変更を識別しない。
folder自動/手動pinの描画結果、ZIP代表、drive pin、SearchContainer代表、
thumbnailのメモリcache・catalog read/seed・in-flight requestの同じcache契約に
policy fingerprintを含める。古いfolder WebPを設定変更後に再seedしない。
個々の画像decode cacheは内容不変なので捨てない。既存catalogを全削除しない。
fingerprintの正本も§3のownerとし、保存形式を追加するときは旧proofを無効扱いで再生成する
後方互換を設計レビューする。paintで再走査/DB/検査をしない。

### 5.2 Ctrl+Up/Downと画像本判定

`FolderTreeOptions` / DFS候補 / stop predicate / 先頭ページ探索に同policyを渡す。
除外archiveを候補に入れず、通過する直接mediaがないfolderはskip対象にする確定仕様（Q6、2026-10-08）。
子folderがあるだけで走査を打ち切らず、従来DFSを継続する。直接クリックは空folderにも入れる。
skip_limitを使い切った通常gridの既存fallbackと、fullscreenの境界停止は保存する。
「絶対に空folderへ止まらない」ために無制限DFSを追加しない。

確定した画像本判定は「ポリシーと同名処理後の、表示対象となる認識mediaが非空で全て画像、
表示対象の子コンテナなし」。画像+非表示動画なら画像本になる（Q7）。
Folder構造は保持するので実子folderがあれば従来どおり非本。全画像除外は非本。
通常openの`scanned_folder_is_image_book`、page-count worker、sub展開の畳み込み、
Remote読順を同じ投影と述語に揃える。`image_page_recognition_fingerprint`もpolicy値を含める。
保存済みmeter比率は次の読書記録まで維持する。位置復元は現行kind検証だけでは安全にできない。
読順への投影と永続identityの確定仕様は§5.5/Q15で扱う（2026-10-08）。
非表示pageを必須targetとしたbookmark/検索復元は先頭へのsilent fallbackをせず、理由を通知して拒否する
確定仕様（Q3b、2026-10-08）。単体画像/動画/音声の外部openは維持し、兄弟一覧だけに適用する。

### 5.3 件数・badge・facet・metadata

一覧件数、表示/選択/check、検索hit_count、stack count、Smart結果数、
本としての有効ページ数は通過後の同じ列を参照する。総登録数はcollection/bookmark等のDB全件数として
区別し、「表示X / 登録Y」などで隠した登録が消えたと誤認させない。
folderの`OmittedFolderEntryCounts`に種類設定の内訳を追加する既定（D8）。
いまあるhidden/same_name/unsupportedと二重計上しない。badge描画でFSを再計数しない。
facetの候補・値別件数は基礎一覧の通過後だけを母集団にする。

表示設定はデータ削除ではない。tags.db、rating.db、bookmarks、collection手動順/ID、
reading_history、sidecar、編集・pin、索引のingest/export/backup/rename migrationを変更しない。
media用JSON/TXT/XMP、`mimageviewer.dat`、manifest等は一覧対象形式とは別の補助入力として
従来どおり読む。隠したファイルのtagを消したり、metadata cleanupのmissing根拠にしたりしない。
collection手動並べ替えは§5.4の全登録順を保持し、隠れたentryを削除しない。

### 5.4 collection並べ替えの全登録順と表示projection（r3追加）

現行は全entryを画面stateに保持して全IDを保存する。subset保存はDBがInvalidOrderとして拒否し、
以前記載した「既存merge owner」は存在しない。**新しい純粋merge adapterを設計する**。
snapshotの全順序Fとrevisionを編集stateの正本に残し、表示index Vを同じFileTypeVisibilityで作る。
選択/dragはstable IDでVに限る。移動結果の可視ID列V'はVと同じID集合であることを検証する。
Fのうち可視IDが占めたslotだけを左からV'で置換し、非表示IDのslot/相対順を保つ。
例: F=[A.jpg,H.png,B.jpg,J.png,C.jpg]、PNG除外でCを先頭へ移動すると
全保存順=[C.jpg,H.png,A.jpg,J.png,B.jpg]。DBへこの全ID列と元revisionを送り、
既存exact-set/revision検証を残す。並べ替え中はモーダル化して設定変更を許可しない案とし、
policy変更やrefresh後に古いVを新snapshotへmergeしない。rename/delete/conflictは既存refreshを使う。
新しいDB API、隠れたID専用保存先、部分保存やrollbackは作らない。
編集画面だけ全登録表示を維持する簡素化も検討したが、全一覧への適用から例外になるため
既定案にはしない。mergeの非表示slot固定は利用者が採用した確定仕様（Q17、2026-10-08）。

### 5.5 ページidentityによる復元と旧raw記録（r3追加、Q15）

現行resume_page_for_containerはidxが画像kindなら採用するだけ。
`1.jpg / 2.png / 3.jpg`のraw index=1を保存後、PNGを除外して同じ1へ戻すと3.jpgに誤着地する。
「現行検証で有効なものだけ採用」という以前の記載は撤回する。

読書記録ownerでraw indexと一緒にoptionalな論理PageIdentityを保存する確定仕様（Q15、2026-10-08）。
通常画像は既存path key、書庫は論理元container＋完全entry_name、PDF/EPUBは論理元文書＋page_num、
製本は既存page identityを使う。変換ZIPの物理cache pathを新たな論理identityにしない。
現在の読順projectionはPageIdentity→表示indexの対応を準備workerで作る。
復元は保存identityが同じ列にある場合だけそのindexを使い、除外済みなら理由を通知する。
同じindexの別画像や最も近い画像を代用しない。保存container keyと従来raw列は残し、
新optional列の追加/default/旧版との互換をbook_resume_dbの設計・移行テストに含める。
sort/page追加・削除にもidentity照合を使うが、内容変更後も同一ページと保証する設計には広げない。

#### 5.5.1 raw-only更新によるidentity失効（r4追加）

現行BookResumeDb::setは`INSERT(path,page) ... ON CONFLICT ... UPDATE SET page`だけを
書き、BookResumeWriterの既存upsertもpageとmeterだけを書く。旧版が追加列を知らないまま
既存行を更新すると、以前のPageIdentityは残る。「identityが存在する」だけでは最新記録といえない。
raw値の一致や保存時rawの併記だけでも、同じindexが別のページを指す場合を検出できない。

設計既定はDB自身で失効を保証すること。identity列の追加と同じmigrationで
`AFTER UPDATE OF page ON book_resume` triggerを作り、NEW.pathのidentityをNULLにする。
`OLD.page != NEW.page`の条件は付けず、同じraw値を再保存した場合も必ず失効させる。
旧版のpage-only upsertはこのtriggerを残して実行するので、旧版コードの変更は要らない。
新規INSERT/REPLACEのraw-only行はidentity列のNULL defaultで未同定になる。
read側はNULLを§5.5の旧raw記録と同じ扱いにし、古いidentityを復活・推測しない。
path key、page列、meter列の意味は変えず、triggerはidentity列だけを変更する。

新版writerは同じ専用workerの**一つのtransaction**内で、まず従来のraw/meter upsertを実行して
identityを失効させ、次に`UPDATE ... SET page_identity=? WHERE path=?`で同じ読書イベントの
identityだけを保存する（後段はpage列を書かない）。旧版・raw-only APIもこの失効契約を通す。
rawとidentityを同じupsertに書く案はAFTER triggerで新版identityまで消すため採らない。
readerはcommit済みの対だけを読む。trigger/列のmigrationは一括transactionにし、writerの全
page更新入口を照合する。新しいtimestamp、世代counter、旧版検出・再試行stateは増やさない。
既存のバックアップや削除・全消去は行と一緒にidentityを扱う。旧DBを戻した場合もmissing列の
migration後はNULLから開始し、以前の新版DBのidentityを別保存先からmergeしない。

実装時の互換回帰は**新版→旧版→新版**を同じ実DBで通す。新版で2.png/raw=1とidentityを記録、
旧版の実page-only SQLで3.jpg/raw=2を保存し、新版再openでidentityがNULL、最新raw=2を読むこと、
除外ありでは古い2.pngへ戻らず旧raw通知へ進むこと、除外なしでは従来raw=2を使うことを検査する。
さらに列変更/投影で別ページが同じraw=1にあるケースも旧版SQLで保存し、値が同じでもNULLになること、
新版の続くidentity付き保存がcommit後に復元可能になること、raw/meterが失効triggerで変わらないことを検査する。
これは旧版更新後の記録整合性を守る設計契約であり、Q15の確定した復元制約と共に維持する。

identityのない旧記録から過去ページを確実に復元することはできない。
種類除外が読書ページ列に適用されるsessionでは旧raw indexを自動採用せず、通知して利用者の選択から
再記録する確定仕様（Q15、2026-10-08。除外設定を戻した従来列では現行raw復元を維持）。
例えばPNG除外後は「以前の位置を特定できません」を出し、3.jpgへ勝手に移動しない。
一時的な旧記録復元の制約と追加の永続列の費用を含め、利用者がQ15の推奨どおり採用した。
全候補の旧順序を再現してraw→identityを推定する案も検討したが、過去のsort/同名候補集合が
保存されておらず、除外により新しく露出した同名JPEGもあるため、確実な移行としては採らない。
閲覧履歴・bookmarkの明示targetは各既存identityを同じ投影で照合する。§1.328のanchor問題とは分離する。

## 6. 永続化とUI案

新しい設定fieldは`Settings.file_type_visibility`のみ。既定は全対応形式を表示。
読書位置DBの追加identity列とraw-only失効契約は§5.5で扱う。
settings.dbの既存Settings carrierへ追加し、旧fieldの意味・既存facet値を移行しない。
新設定は未出荷、旧設定ファイルからのmissing fieldはdefaultで読み込む。
古い版へ戻したときの未知field保存/互換判定はSettings DBの現行方針を使い、
schema/tableを別途新設する理由はない。enum未知値を空集合へ黙って変換しない。

`settings_transfer.rs::preferences_policy!`の**export対象**へ「表示するファイル種類」として
明示分類する既定（D9）。パス・ユーザーデータを含まない全体環境設定なので別PCへ転送可能。
validated parse、default/roundtrip、全field分類、import失敗で元値不変、
preferences OK/Cancel、backup/recoveryの境界を検査する。
favorite_view_overlay、A/B記憶、起動一覧recordには追加しない。

環境設定「ファイル処理」に独立項目を置く案。
category checkbox＋拡張子詳細、全表示へ戻すボタン、除外適用の説明を示す。
既存「書庫を無視/確認/変換」とは別設定で、表示を許可しても開封処理が許可されるとは限らない。
toolbar quick導線は同じ設定を開く任意登録ボタンと適用中の印を推奨し、
一時解除boolや第二のeffective policyを増やさない（D10）。
「全部表示」操作も採用するなら同じ永続設定の変更として扱い、元状態stashを作らない。
新キー操作を付ける場合は`KeyAction`とkeymap helper一式、既定割当なし。raw keyイベントを追加しない。

Remoteは同じ設定を本体から読む。端末側に設定編集UI/APIを増やさない既定（D1）。
各既存prepared/read cache identityにpolicy値を含め、次のlist/read要求で最新設定と照合する。
payload説明を増やすなら`crates/remote-ipc`のprotocol更新を同時に行う。

## 7. 状態の組み合わせを減らす検討

**確定仕様**: 設定のOKで一回確定、既存reload/prepareで基礎一覧を作り直す。
prefs draftと表示policyを混ぜず、Cancelなら何も変えない。
同名処理/書庫処理変更の既存reload経路を調べて再利用し、live retain用の専用state machineを作らない。
検索・sub展開・snapshotは既存close→元場所reloadを使う確定仕様（Q11、2026-10-08）。
閉じると検索結果/一時snapshotが失われる影響を含め、利用者が推奨どおり採用した。
collection/Smartは保存定義から既存再prepareできるのでその経路を使う。

設定変更中の各detached viewerを閉じて作り直す案も検討した。
動画再生や開いている本を中断する既存挙動の削減になるため、この案は採用しない。
開いているviewerの読書/再生contextと読順をその終了まで保持し、
次の一覧生成/open/reloadから新policyを使う確定仕様（Q12、2026-10-08）。
新policyを適用した一覧へ戻る境界では除外済みのselected/checkを持ち越さない。
この例外は「すべての新しい一覧生成には同policy」を維持し、開いているsessionの寿命だけを区切る。
即時に全windowを更新する要求なら、既存context採用/worker cancel/cache所有境界を設計レビューしてから
別chunkで実装する。mounted以外のcontextをmain経由で一括resetしない。
detached述語/viewport変更に達した場合はrework§2の合意と§11記録が必要。

長い変換・password・保存は既存モーダルを維持し、その途中でprefs適用を許可しない。
追加のrollback・supersession・resume pendingは作らない。
rare cache/DB failureの多段回復は追加しない。現行ログ/通知/次回再生成の範囲で扱い、
利用者の設定や登録データを落とす割り切りはしない。

## 8. 確定した設計既定と利用者仕様（2026-10-08）

### 8.1 決定した設計既定（利用者が上書き可能）

以下はr3/r4で独立レビュー助言を採用した設計既定。
2026-10-08の利用者決定でQ1/Q2/Q4/Q8/Q9/Q10/Q13/Q14をそのまま維持すると確認した。

| ID（旧質問） | 決定した既定 |
| --- | --- |
| D1（Q1） | Remoteの設定編集UI/APIは追加せず、本体で共通設定を変更する |
| D2（Q2） | 実フォルダ・ドライブは通路として保持し、ファイル形式だけを除外する。内部書庫のZipDirは§3.1で区別 |
| D4（Q4） | 同名優先は表示対象候補だけで判定する。動画除外なら同名JPEGが残る |
| D8（Q8） | 種類設定による非表示を件数内訳に追加し、登録数と表示数を区別する |
| D9（Q9） | settings_transferの環境設定export/import対象に分類する |
| D10（Q10、r4） | 環境設定＋任意登録toolbarボタンで同じ設定を開く。一時解除toggle/stashは作らず、「全表示」も同じ永続設定を変更 |
| D13（Q13、r4） | categoryと個別拡張子を両方選べるUI。RAW全体またはCR2だけ除外でき、PDF/EPUB・ZIP/RAR/7z/LZHも分ける |
| D14（Q14） | 履歴再入場も通常openと同じく、live親条件を退避して子では条件なし。地点別filter記憶の要望がある場合のみ再相談 |

### 8.2 利用者が確定した仕様（全10件、2026-10-08）

利用者は以下の全質問について、費用・影響を含め推奨どおり採用した。未回答の質問はない。
IDは以前の議論と対応させ、仕様内容はr4の推奨から変更しない。

| 旧質問ID | 確定仕様・具体例 | 維持する条件と費用/影響 |
| --- | --- | --- |
| Q3a | 種類設定を本内部のページ列にも適用する。PNG除外でZIP内1.jpg/2.png/3.jpgから2.pngを除く | 書庫はmember、PDF/EPUBページは元文書形式で判定。読順・ページ数・復元のprojectionが変わるため§5.5/Q15も適用 |
| Q3b | 非表示の本・内部ページは明示指定でも拒否する。PNG除外中のZIP内2.png bookmark、PDF除外中のPDF直接openも対象 | 理由を表示し、他ページで代用しない。単体画像/動画/音声の外部openは維持し、兄弟一覧だけに適用。明示openの例外session案は不採用 |
| Q5 | RAWのfolder代表pinを残し、RAW除外時はJPEG等の自動代表へ表示だけ替える | 候補なしならアイコン。動画のsidecar JPEGは補助資源として使い続ける |
| Q6 | 動画除外で表示mediaがなくなったfolderをCtrl+上下でskipする | 従来skip_limit/fallbackを維持し、直接クリックでは空folderにも入れる |
| Q7 | JPEG＋MP4のfolderで動画を除外すると、JPEGだけの本として自動open/畳み込みする | 非空の表示mediaが全画像で子コンテナなし。全画像除外なら非本。通常/Remote/page countを揃える |
| Q11 | 種類設定の確定時、検索結果・サブ展開・一時snapshotを閉じて元場所へ戻す | 一時結果が失われる影響を受け入れ、既存reloadを使う。各producerの再実行案は採らない |
| Q12 | 動画除外後も再生中の動画を継続し、開いている本の読順もsession終了まで保持する | 次の一覧生成/open/reloadから新policyを適用。全windowへの即時適用なら中断動作と所有変更を再相談 |
| Q15 | ページidentityも保存し、identityのない旧記録・旧版で更新した記録は種類除外を適用する読書sessionで自動復元を見送る（§5.5） | 通知後に再選択・再記録が必要。追加optional永続列、raw-only失効trigger、新版→旧版→新版回帰を用意。raw-onlyで投影後indexを採用する案は不採用 |
| Q16 | 内部書庫の列挙来歴を追加し、旧変換cacheの来歴が必要なときは再変換を求める（§3.1） | ZIP内book.zipを除外してnotes.zipという実folderは残す。旧cacheの再変換に時間がかかる影響を受け入れ、内部書庫を今回対象外にする案は不採用 |
| Q17 | collection並べ替えは非表示entryの元slotを固定し、可視entryだけを入れ替える（§5.4）。A,H,B,J,C→C,H,A,J,B（H/JがPNG） | 全ID保存・revision検証を維持。編集画面だけ全登録を見せる例外案は不採用 |

## 9. §1.339 — 履歴再入場のfacet退避を採用境界へ集約する

### 9.1 観測と根因

利用者報告: **★3一覧 → extension=zip → ZIPを本としてopen → Backspaceでページ一覧 →
toolbar ←で★3へ → toolbar →でZIPページ一覧**が空になる。
元一覧が★3であることは利用者回答（2026-10-07）。実アプリでの本書作成者の観測はない。

コード上は次の非対称がある。

1. `collection_grid.rs`の`GridVirtualOpenIntent`生成は`suppress_facet_filter`を持つ。
   `app.rs::commit_grid_virtual_open_effects`は採用時に条件を退避する。
   `commit_physical_history_transition`のRating ZIP/PDF branchもeffectsがある場合だけこの入口を通す。
2. `start_rating_physical_restore`はtypedな`RatingPhysicalRestore`とnav_chainを持つが、
   通常grid openと同じeffectsを作らない。再入場も含む物理履歴はこれを必須にしていない。
3. `sync_facet_filter_scope` → `update_facet_filter_suppression_for_scope_change`は
   `effective_folder`の親子path比較でだけ新たな退避を推定する。
   ★合成pathとZIPは親子でないためこの救済が働かない。
4. `metadata_ops.rs::path_in_subtree_ci`は区切り統一・小文字化後、anchorをそのままprefixにする。
   `D:/`→`D:/book.zip`では残りが`book.zip`なので`starts_with('/')`を満たさない。
   trailing separator付きfolderにも同型。比較だけ直してもRating再入場は残る。
5. `FolderNavHistoryTarget`は現在地/親provenanceを持つがfacetの退避routeを持たない。
   paint/rebuildでの推定とopen intent側の明示操作に責任が分散している。

不変条件: **採用済み表示scopeに対応するfacetを、初回visible_indices計算より前に確定する**。
親の条件はその子scope内では退避し、親へ戻れば復元する。
同じ子への通常open・履歴←/→・BSの違いで結果を変えない。
reload、fullscreen→同じ本のページ一覧、ZIP内部の同scope移動では二重pushしない。
未採用・失敗・取消・stale・password待ちでは表示中scope/active filter/stash/履歴を変えない。

### 9.2 単一の状態ownerと要求

提案: 現行`facet_filter_scope`と`facet_filter_suppression_stack`の責任を
**App-globalのmain grid専用**`FacetNavigationState`へ集約する。
scope/stackは現行でもApp全体が所有し、ViewerContextBundleに含まれない。
A/Bにもslot別facet stashはない。以前の「既存context runtime内に閉じる」前提は撤回する。
active値の永続carrierは既存`settings.facet_filter`を保持し、
ownerはその値を受け渡す唯一のmutation APIを持つ。別active filter copyを作らない。
新しい`pending_facet_*` / bool / sentinelをAppへ足さない。

この修正では所有範囲を変更しない。既存のdetached physical/independent collection等の
global filter除外を維持し、viewer contextのfork/mount/swap/closeではownerを動かさない。
`rebuild_visible_indices_preserving_facet_scope`とbundle交換の契約を維持する。
context別facetを作るなら共有Settings carrierの分離も必要になり、本件とは別の設計となる。
今回その拡張は採らず、detached viewport/述語の修正も予定しない。

scopeはtypedな論理現在地（通常path、ZIP本+内部book prefix、Rating stars、
Collection ID+entry、Smart ID+position、検索等の一時origin）から投影する。
cache ZIPへのalias変更を別scope entryと扱わず、元書庫identityを使用する。
本内階層のcontainsはzip prefixのcomponent境界で扱い、filesystem pathとの文字列混合をやめる。

`FacetRoute`（提案）は**親→子のscope連鎖だけ**を保持し、設定値のhistory snapshotを持たない。
既存history entry/restore/requestの同じownerにrouteを組み込む。
targetの場所identityとrouteの帰路を分離し、同じpathでもRatingPhysical/通常Pathのprovenanceを失わない。
通常openで成功した親→子edgeを記録し、履歴再入場はそのedgeを再利用する。
単に「宛先拡張子がzipならfilterを消す」boolをhistoryへ足す案は採らない。

共通reducerは採用先のrouteと現在routeの共通prefixを求め、
退出したscope分をpopして親filterを復元し、入ったscope分だけlive親filterをstashして空条件にする。
有効条件がなければsaved-filter frameを作る必要はないが、routeの親子関係自体は保持する。
history entryがrouteを持つので、戻るとstashがpopされても進む先の親子関係は消えない。
親のzip条件を戻って編集した後の再入場は、古いsnapshotでなく**その時点の親条件**を退避する。
子で手動編集したfilterは親stashを書き換えない。親へ出たら従来の親復元を維持する。
独立した場所への移動は既存の条件復元/解除仕様を保ち、すべての場所別filter記憶へ広げない。

route組立は、要求時のsource typed locationと、成功したdestination分類/ownerから行う。
合成親は既存RatingPhysical.nav_chain、Collection entry、Smart.position、search originから得る。
通常folder childは正規化したpathのcomponent包含で得る。drive root/UNC root/trailing separator、
cross-drive、`book`対`book2`を区別し、driveを消すDB keyでpath関係を判定しない。
ZIP/PDF/EPUB/変換書庫は**成功した読込payload**で本entryと分類する。suffixだけで決めない。
アドレス直接openやhistory Pathとしての本openにも同じdestination分類を使い、
sourceが存在する場合に親filterを退避する。検索の一時surface/退出は既存origin routeを使う。

採用時の順序:

1. main gridのnavigation採用であることを既存navigation scope/ownerで確定する。
   detached/parkedの一時mountやread-only context交換は対象外とする。
   既存ownerがsource location / items generation / slot / request / history headの妥当性と
   読込payloadの採用可能性を検証する。失敗があり得る作業を先に済ませる。
2. 既存の可視採用transactionでsource grid位置を保存し、facet reducerを適用する。
   `place_keys`は現在の場所依存条件として既存どおり除去、name-query runtimeも値と同期する。
3. 新locationとitems/metadataをinstallし、確定済みfacetでvisible/order/selectionを計算する。
4. 同じtransactionでhistory/current/BS provenanceをcommitする。
   reducerの実行後に採用拒否となる配置は許さず、rollback専用状態を足さない。

`sync_facet_filter_scope`のframe中path推定は撤去し、scope一致の検証か純粋なprojectionだけにする。
現行open helper群は共通採用要求へのadapterに縮め、退避pop/pushを個別に行わない。
ZIP階層の`maybe_restore_facet_filter_after_zip_level_change`も同reducerへ接続する。
filter UI編集もowner経由でruntime同期する。settings保存時期は現行互換を守り、
新しい永続history/filter table、DB migration、画面描画時のDB読み取りは作らない。

### 9.3 横断する入口・終了と簡素化

| 経路 | 保証 |
| --- | --- |
| 通常folder/drive root → child、本tile → ZIP/PDF/EPUB/RAR/cache ZIP | 初回openと再入場が同じrouteを採用。direct/warm/coldの違いでscopeが先行しない |
| Rating/Collection/Smart/Tag/Search/History/Bookmarks/Sub展開から本open | typed origin/親chainでentryを確定。物理path親子関係を要求しない |
| toolbar/キー/マウスの←→、BS、アドレス、A/B | mainの既存routerを保ち、成功した採用を同じmain ownerへ送る。A/Bは採用されたmain場所のrouteとして扱い、slot別stashやfilter値コピーは新設しない |
| 本内ZIP階層/入れ子/単一wrapper collapse、fullscreen→ページ一覧 | prefix scopeを使い、同じscopeでは何もしない。退出したbook分だけ復元 |
| reload/notify/ソート/同場所再install | route不変なので二重stashを作らない |
| conversion/password/sidecar待ち、cancel/error/superseded、close | 未採用要求のdiscardだけ。既存モーダル/取消ownerを維持。成功までactive値とstashに触れない |
| F12/複数viewer | facetはmain専用App-globalのまま。fork/mount/swap/closeはscope/stack/Settings.facet_filterを変更せず、detachedのglobal filter除外を維持。bundleへfacetを追加しない |

簡素化の検討: 履歴の→を禁止する、facetを常時解除する、ZIPを開くたび全stashをclearする案は
既存機能を削るため不採用。全場所のfilter snapshot永続化も要求外で、親編集との古い値競合を増やすので不採用。
既存のモーダルを維持して変換中の割込みを増やさず、既存採用transactionと親chainを再利用する。
新ownerは既存runtime fieldsを置換するもので、追加の並行状態ではない。
§1.345とは設定・owner・受入テストを分離し、1.339単独の差分として先に実装できる。

### 9.4 履歴再入場の設計既定（D14、旧Q14）

通常openと同じくlive親条件を退避し、子は条件なしから開始することを決定した既定とする。
子/履歴地点別の過去filter記憶は追加しない。利用者がその追加を要望した場合にだけ再相談する。

## 10. 受入条件・実装順・引継ぎ

§1.339を独立chunkにし、D14とmain専用所有の設計owner/独立reviewerの構造合意後に実装する。
まずreported routeを本番history handlerでredにし、zip内jpgが空にならないこと、
戻ると★3のzip条件が復元すること、繰り返してもstash深さが増えないことを検査する。
source suffixへのguardだけでは通らない同じZIPへの別親provenanceの対照を入れる。
drive/UNC root、末尾区切り、cross-drive、back再入場、PDF/EPUB/convertedのlogical alias、
ZIP nested prefix、folder reload、親条件編集、name-query/placeKeys、取消/失敗/stale、
A/Bのmain採用と、detached/parked mount・swap・closeでmain ownerが不変であることを
純粋reducer＋採用handlerで検査する。slot別stashが既存だという前提のtestは作らない。
§1.328選択・可視性、起動復元、rating sort、search退出の既存回帰を併用する。

§1.345は§8.2の確定仕様に従い、分類/Settings転送 → folder/virtualと派生predicate →
aggregate producer → Remoteとcache/reloadの順でcoherent chunkに分ける。
部分producerだけを公開して「app全体対応」としない。未接続があれば内部実装段階のまま引き継ぐ。
必要なテスト:

- all-visible default、category/extension/RAW/EPUB/仮想member分類、同名競合順、AppleDoubleとの内訳。
- §4各producerの同fixture投影、aligned meta/binding/ID、stack/検索countsと代表が除外済みになること。
- image+video非表示の本判定、全部画像除外、子folder、Ctrl+上下skip/fallback、folder pin/auto/catalog再利用。
- Settings旧値/default/roundtrip/export-import/Cancel、policy変更後のstale完了・reload・snapshot復帰。
- data rowsとsidecar/tag/collection順が不変、local/Remoteの表示列・読順・count一致、Remote prepared cache更新。
- 2.png保存→PNG除外で3.jpgへ誤復元しないこと、identity/旧raw/除外target/設定復帰、永続列の互換。
- 新版identity保存→旧版raw-only更新→新版再open、同raw値の旧版再保存でも失効、以後の新版保存でidentity再確定（§5.5.1）。
- 同suffix実directoryと実書庫、同prefix alias、複数段のcontainer ancestry、旧変換cache manifest欠落。
- collection全ID集合、非表示slot/相対順、可視drag後の全ID保存、設定変更・revision conflictで旧投影を保存しないこと。
- headless UI snapshot（prefs・適用中表示・件数）、full lib、fmt、通常/portable core check、glyph。

実機確認はcoordinatorが具体的なシナリオ・時間・入力/使い捨てdataの範囲を提示し、
利用者の明示承認を得る検証枠へ回す。製品バイナリをこのworktreeの実装担当は起動しない。
bf509352dの実装・設計への独立レビュー承認は利用者連絡に基づき§12へ記録した。
実装担当のコード前提照合は独立reviewの代わりではない。

## 11. r3指摘への対応記録（2026-10-08）

利用者が提示した独立レビューの4設計指摘は全てコードと一致し、採用した。反対意見はない。
raw-kind検証では投影後identityを守れない点を§5.5/Q15へ、facetのApp-global/main専用所有を§9へ、
ZipDirに来歴がない点を§3.1/Q16へ、collection subset merge ownerが存在しない点を§5.4/Q17へ訂正した。
Q1/Q2/Q4/Q8/Q9/Q14は上書き可能な決定した設計既定へ移した。
残る質問は§8.2。今回も設計のみであり、§1.339/§1.345のコードや永続列を実装していない。

### r4補完（2026-10-08）

page-only upsertが未知のidentity列を残す指摘は現行SQLと一致し、採用した（反対意見なし）。
§5.5.1にDB triggerによるraw-only更新の失効、新版writerの一transaction保存、
新版→旧版→新版と同raw値更新の回帰契約を追加した。永続コードは変更していない。
Q10/Q13は利用者が上書きできる推奨設計既定D10/D13へ移し、Q3は本内部適用Q3aと
明示open拒否Q3bへ分割した。Q15には旧版で更新した記録の制約も明記した。

## 12. 利用者決定と独立レビュー承認の記録（2026-10-08）

利用者はQ3a/Q3b/Q5/Q6/Q7/Q11/Q12/Q15/Q16/Q17の全10件を、r4の推奨どおり採用した。
Q1/Q2/Q4/Q8/Q9/Q10/Q13/Q14の設計既定もそのまま維持する。費用・影響・例外を含めた
確定仕様は§8に記録し、本文の回答待ち表現も同じ決定へ揃えた。未回答の質問はない。

同日の利用者連絡により、独立reviewerがbf509352d（完全hash:
bf509352d8240b6d14c368a8fb4511e6ee64e156）の実装・設計を承認したことを記録する。
この記録作業は新たな独立レビューではない。今回の変更は決定・承認の記録のみで、
設計内容・受入条件・実装順を変更せず、§1.339/§1.345の実装や製品起動も行わない。
