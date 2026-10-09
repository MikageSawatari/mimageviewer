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
6. 実装前提照合（2026-10-08）で、**通常folder履歴には成功採用transactionが未整備**と分かった。
   `dispatch_main_folder_history_input`の通常Path branch（`app.rs:22969`）は
   `navigate_folder_history_back/forward`で先にstackをpop/pushし、`target.into_path()`を返す。
   `ClassifiedOpenContinuation::DirectNavigation`（`app.rs:890`）に残るのはPathとrollback snapshotで、
   typedなreplay宛先、source location/items generation/slot-switch sequenceではない。
   classificationのpollもrequest/context/cancelの検証であり、physical履歴のsource/history検証とは異なる。
   「全入口で既存transactionをそのまま再利用できる」という旧§9.2の前提を撤回する。
   以下はcoordinator指示による設計改訂であり、通常folder側の要求移行も§1.339の実装範囲とする。
   改訂前のbf509352dへのレビュー承認を、この追加構造の承認とは扱わない。
7. 再レビュー（2026-10-08）の2指摘をcodeで確認した。sidecarは`current_folder`とitemsを
   installした後に開始し（`app.rs:36240/36317/36628`）、新items/世代を所有証明に使う。
   既存continuationを「旧表示を保持する未採用prepare」として流用する前提は撤回する。
   また`SmartFolderSourceLease`（`smart_folder.rs:749`以降）は安定したsurfaceの意味を検証し、
   PDF verificationによるitems世代変更やCollectionの同一owner内revision更新を意図的に許す。
   共通要求で全sourceにitems世代一致を重ねる設計も撤回する。両指摘を採用し、反対意見はない。

不変条件: **採用済み表示scopeに対応するfacetを、初回visible_indices計算より前に確定する**。
親の条件はその子scope内では退避し、親へ戻れば復元する。
同じ子への通常open・履歴←/→・BSの違いで結果を変えない。
reload、fullscreen→同じ本のページ一覧、ZIP内部の同scope移動では二重pushしない。
navigationの未採用・採用前の失敗/取消/stale・password待ちでは、その要求は
表示中scope/active filter/stash/履歴を変えない。採用後sidecar hydrationはこの未採用状態に含めない。

### 9.2 単一の状態ownerと要求

#### 9.2.1 状態を減らす案の比較と選択

transactionを足す前に、CLAUDE.md「設計の簡素化」に従い次を比較した。

| 案 | 判断と理由 |
| --- | --- |
| stashを宛先path/identityだけで引き、採用時だけ解決する | **採用時解決は採用、pathだけのmapは不採用**。同じZIPを★3、通常folder、Collectionの別entryから開く場合は帰路が違う。path単独ではどの親条件を戻すか決まらず、親条件編集後の再入場にも古い値を使ってしまう。場所ごとのfilter記憶はD14にも反する。originを含むtyped routeで帰路を区別し、filter値はlive ownerの退避frameだけが持つ |
| stackのpop/pushを成功したloadの後へ移す | **採用**。pending中はcommitted stackを変更しないので、通常履歴の「stack変更済み＋旧表示＋rollback待ち」をなくせる。単一採用点でfacet、items、履歴を確定し、失敗時は要求をdropするだけにする |
| 通常folder履歴だけ新しいtransaction manager/全bundle複製を作る | **不採用**。Folder scanを扱える既存`PhysicalHistoryTransition`/preflightを拡張する。分類・scan・PDF列挙・変換の既存ownerへ一つのtyped要求をmoveし、新しいApp pendingや表示rollback用bundleを増やさない |
| sidecar完了まで宛先の採用を遅らせる | **不採用**。既存sidecarは採用済みitems/世代を使う。遅延採用にはoffscreen items/key/世代を扱う別契約が必要で、現行復元・DB反映・first-display tailまで改修が広がる。先に移動を採用し、その宛先をhydrateする既存方式を維持する（§9.3.3）。sidecar待ちをnavigation rollbackのphaseにしない |
| 通常のscanまでモーダルにして履歴連打・別openを止める | **不採用**。通常閲覧を遅くし操作を削る。変換/passwordの既存モーダルは維持し、通常pendingは既存の取消・置換で扱う。連打の仮cursorは既存Smart履歴の方式を同じ要求内へ統合する（§9.3.2） |
| filterを毎回clear、forward禁止、stash全消去、全地点snapshot永続化 | **不採用**。公開済み機能を削るか、親編集と過去値の組合せを増やす |

選択は**typedな宛先/帰路を一つの既存読込要求に保持し、stack更新を成功後へ遅らせる**こと。
stashのための別transactionは作らない。要求中に保存するのは場所・帰路・履歴操作・所有証明で、
active filterやstashのrollback copyは持たない。読込失敗へのretry/回復phaseも追加しない。

#### 9.2.2 採用済みfacetのownerと履歴entry

現行`facet_filter_scope`と`facet_filter_suppression_stack`の責任を
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
履歴vectorのentryを`FolderNavHistoryEntry { location: FolderNavHistoryTarget, route: FacetRoute }`
という一つの値にする（型名は提案）。normal/A/Bのback/forward、history snapshot、
Smartの仮cursor、要求のsource/targetは同じentryを受け渡す。`FolderNavHistoryTarget`の
既存typedな場所表現は保持し、route用の別mapを作らない。
targetの場所identityとrouteの帰路を分離し、同じpathでもRatingPhysical/通常Pathのprovenanceを失わない。
同一地点reload/dedupの判定は既存location比較を維持し、採用した新しいentryで帰路を更新する。
source/history headの所有検証ではroute込みのentryを比較する。履歴の読み出しでlocationを表示用に
投影することはできるが、読込要求へ渡すときにentryを`PathBuf`へ潰してはならない。
検索等のreturn originも既存restore payloadにrouteを添えて保持し、退出要求へmoveする。
routeはruntime情報で、SettingsのA/B保存path、MRU、起動復元target、DB keyの形式は変更しない。
通常openで成功した親→子edgeを記録し、履歴再入場はそのedgeを再利用する。
単に「宛先拡張子がzipならfilterを消す」boolをhistoryへ足す案は採らない。

共通reducerは採用先のrouteと現在routeの共通prefixを求め、
退出したscope分をpopして親filterを復元し、入ったscope分だけlive親filterをstashして空条件にする。
有効条件がなければsaved-filter frameを作る必要はないが、routeの親子関係自体は保持する。
history entryがrouteを持つので、戻るとstashがpopされても進む先の親子関係は消えない。
親のzip条件を戻って編集した後の再入場は、古いsnapshotでなく**その時点の親条件**を退避する。
子で手動編集したfilterは親stashを書き換えない。親へ出たら従来の親復元を維持する。
独立した場所への移動は既存の条件復元/解除仕様を保ち、すべての場所別filter記憶へ広げない。
具体的には、退出するedgeのsaved frameを内側から復元して消費し、独立先へは復元後のactive条件を
引き継ぐ。本/子へ新たに入るedgeだけがそのlive条件を退避する。退出した子での条件は親stashを
上書きせず、親saved frameを復元する場合に置き換えられる。親条件が無効でframeを作らなかった
場合や、手動復元でframeを消費済みの場合は、復元対象がないのでactive条件を無条件clearしない。
この場合の子で編集した条件の引継ぎも現行互換とする。
場所変更ではactive/frame内の`place_keys`を従来どおり除去する。
同じrouteのreload、ソート、通知、cache alias変更は退避・復元ともno-op。
saved frameはentered scope/route内のedgeに対応付ける。有効な保存条件がないedgeもrouteには残すので、
frameの有無で現在地を推定しない。消費済みframeを再入場まで作り直さない。

「親絞り込み退避中」バッジのクリックによる手動復元（`ui_main.rs:12180`）も維持する。
同じownerが直近saved frameを一回だけ消費しactive値/name runtimeを更新する。
現在routeや履歴cursorは変えず、その場所での再描画が再退避を起こさないようにする。
親へ退出するときは残存frameだけを復元する。filter UI編集/name入力もownerを通すが、
検索入力debounce、IME helper、Settings保存時期、手動復元の保存は現行互換を維持する。

route組立は、要求時のsource typed locationと、成功したdestination分類/ownerから行う。
合成親は既存RatingPhysical.nav_chain、Collection entry、Smart.position、search originから得る。
通常folder childは正規化したpathのcomponent包含で得る。drive root/UNC root/trailing separator、
cross-drive、`book`対`book2`を区別し、driveを消すDB keyでpath関係を判定しない。
ZIP/PDF/EPUB/変換書庫は**成功した読込payload**で本entryと分類する。suffixだけで決めない。
アドレス直接openやhistory Pathとしての本openにも同じdestination分類を使い、
sourceが存在する場合に親filterを退避する。検索の一時surface/退出は既存origin routeを使う。

`sync_facet_filter_scope`のframe中path推定は撤去し、scope一致の検証か純粋なprojectionだけにする。
現行open helper群は共通採用要求へのadapterに縮め、退避pop/pushを個別に行わない。
ZIP階層の`maybe_restore_facet_filter_after_zip_level_change`も同reducerへ接続する。
filter UI編集もowner経由でruntime同期する。settings保存時期は現行互換を守り、
新しい永続history/filter table、DB migration、画面描画時のDB読み取りは作らない。

### 9.3 横断する入口・終了と簡素化

#### 9.3.1 通常folderのtyped要求への移行

共通の値`MainListNavigation`（提案名）は、source entry/route、要求ごとの`SourceProof`、
宛先entryまたは未分類のtyped location候補、
履歴操作、selection/restore intentを保持する。履歴操作は`Direct / Replay / Restore / SameLocation`の
排他enumにする。Directは成功時にsourceをbackへ積んでforwardを消す、Replayは指定方向の
仮cursor操作を確定、Restoreは本内/検索等の外側履歴に記録しない移動・復元でcursor不変、SameLocationは
reload/ページ一覧復帰でcursor/route不変とする。BSをReplayと混同しない。

この値は**要求を受け付ける時点、分類workerを開始する前**にcaptureする。
現行のclassification後に初めてsourceをcaptureする配置を残さない。
既存request ID/cancel/worker receiverは各読込phaseが保持し、navigationの所有証明と
宛先entryは同じ値をmoveして引き継ぐ。phaseと別のApp `pending_facet_*`は設けない。

**`SourceProof`は行/prepare snapshot依存と、安定したsurface依存を区別する排他enum**とする。
共通headerにitems generationやCollection revisionの一律一致条件を別途置かない。
context/request、main採用資格、slot等の検証も、その要求が既存ownerから引き継ぐ証明を使う。
2026-10-08 の第4回proof監査の決定: 後続の分類continuation／準備／採用がコピー済み宛先・effectsだけを
読む要求はSurface、現在の行/index・sourceのprepare snapshotを読む要求はRowとする。
拡張子・origin名・移動先の種類で選ばない。native row/revision validatorは別に維持する。
既存`quick_folder_switch_sequence`をすべての証明が照合する共通switch epochとして使い、
Quick Folderの同slot再選択／同target A→B→Aと検索owner/query/drill切替で進める。
同queryの結果追加、PDF verification、Collection revisionでは進めず、Surfaceを無通知で取消さない。
新しいepoch fieldやSmart lease fieldを増やす案と、すべてのrow更新を待ち中に凍結する案を比較し、
既存sequence＋切替受理時の既存取消終端を選んだ。追加pending/rollbackは不要である。
2026-10-08 coordinator決定: committed warm PDF verificationの既存保留は維持する。
履歴／分類要求がpending、またはdocument-open modal中は補正をpollせず、採用／取消後に既存ownerで
処理する（2026-09-28導入、3729208d7 / 4640930d2）。この限定した保留で状態の組み合わせを減らし、
並行処理を拡張しない。Surface証明は、その他の到達可能な同owner行更新を理由に要求を取消さない。
RatingPhysicalの既存intentも同じ規則に従い、Explicit／行順snapshotにも使うRefreshの行依存と、コピー済みRestoreを区別する。
Restoreのsource意味identityは元の共通Surface証明が検証する。native validatorではsource items世代や
source地点のCollection revision／viewport hint等の表示snapshotを再照合せず、context／surface／slot／
switch epoch／target pathを維持する。Explicit／Refreshのsource行世代・地点snapshotの一致は維持する。
BSのコピー済み親chainはRestoreとして受け付け、外側の履歴操作は従来どおりDirectで成功後に記録する。
QuickFolderSwitchはコピー済みtarget slot/pathだけを使うためnative items世代条件と不要な保存fieldを除く。
context／surface／slot／switch sequence／path条件は維持する。
全producer/consumerと退役境界のcode監査は [async-architecture.md](async-architecture.md#source-proof選択箇所の監査2026-10-08) に記録する。

| sourceの証明 | 保持・再検証するもの | 許容しない共通化 |
| --- | --- | --- |
| 行/index/prepare snapshotに依存する要求 | 既存Physical/Ratingのsource proof、Collection row/physical owner等を同じvariant内に保持。既存契約にあるcontext/surface/items世代、slot/sequence、選択itemのstable key/path、Collection stamp/entry ID/accepted・wanted revisionをそのvalidatorで検証。消えた行や古いindexを新行へ読み替えない | items世代やrevisionの検証を一括削除してSmartと同じにすること |
| surfaceの意味に依存する要求 | Smartの`SmartFolderSourceLease`をそのまま使う。context、surface generation、quick slot、Folder path / Search種類・query・executed/location / Smart ID・position / Collection ID・positionという既存意味を照合。source行やaccepted revisionを暗黙の追加条件にしない | 同じownerでPDF placeholderが検証されitems世代が進んだだけ、またはCollection revisionが更新されただけで要求を失効させること |

Smart要求は既存leaseを保持して分類・root prepare・child preflight・採用までmoveし、
途中でcaptureし直して別ownerを許可しない。同IDのCollection再open、source path/query/position変更、
context/slot変更は現行leaseが拒否する。一方、同ownerの行/metadata更新はnavigationの失効ではない。
選択indexを使わずcapture済みのlogical pathで進む既存Smart要求へ、後からrow proofを重ねない。
宛先自身のSmart定義、Collection entry/catalog/revision等の準備検証はsource証明と別の目的で維持する。
たとえばCollection行からのphysical openはrevisionを検証するが、Collectionから別Smartを開く
surface依存要求はsourceのrevision更新を理由に拒否しない。

`source entry`は履歴へ記録する帰路であり、`SourceProof`とは用途が違う。
surface variantで`folder_nav_current_target() == captured_entry`を無条件に追加すると、
revision/anchor等の表示hint変化が再び失効条件になり得る。既存leaseが照合する安定した意味と
facetのsource routeを検証し、同じ場所のpresentation更新で帰路を別navigationと誤認しない。

| 現行入口/実行先 | 移行後の受渡し |
| --- | --- |
| `dispatch_main_folder_history_input`の通常Path branch | headをpeekしてReplay要求を作る。`navigate_folder_history_back/forward`を先に実行せず、`Option<PathBuf>`で宛先を返す経路を廃止。toolbar、KeyAction、mouse、ring/gamepadで同じtyped要求をdispatchする |
| 通常folderのBS/親ボタン、`resolve_grid_parent_nav`、`resolve_return_to_parent_nav` | 現行の親優先順位と選択anchorを保持し、Direct要求として渡す。親を求めた時点の`select_after_load`等の副作用は要求のselection intentへ移し、採用時だけ適用。root→DriveListもtypedな成功採用へ接続する |
| 通常folder履歴/親の実folder読込 | `PhysicalHistoryIntent::Navigation`をDirect/Replay/Restore/SameLocationのtyped要求へ置換・拡張する。既存`PhysicalHistoryTransition`と`PhysicalHistoryPreflightPayload::Folder(ScannedDir)`を再利用し、workerでscanする。prescan/分類で得たscanは同じ要求へmoveし、二重scanしない |
| `OpenPathClassification` / `ClassifiedOpenContinuation::DirectNavigation`・`Physical` | 共通navigation値をmoveで保持し、分類完了で同じ値をPhysical要求へ移す。request/context/cancelに加えてcapture済みのSourceProof variantと履歴proofを再検証。通常履歴用`history_nav_rollback`は置換する |
| Direct/warm/cold PDF、EPUB/書庫変換 | warmの初回placeholder採用を待たせず維持する。`DirectPdfAdoption`等の既存adoption payloadに同じnavigation値をmoveする。coldは成功列挙まで未採用、warmは初回採用で消費して後のverificationはSameLocation。変換/passwordの既存continuationはrequest IDを参照し、navigationを二重所有しない |
| Rating/Collection/Smartの既存要求 | 既存typedな場所/準備phase/親chainを維持し、source/history情報を共通navigation値へ集約して同じ採用APIを呼ぶ。別のpending ownerを併設しない |
| 本内ZIP prefix、検索のdrill/戻り、resident Smart root復帰 | worker不要なら準備済みitems/metadataとtyped routeを同じ採用APIへ直接渡す。ZIP内部だけの移動は外側folder履歴を変更せず、成功したprefix/本edgeだけをfacet reducerへ送る |

既存synchronous通常openもscan等が成功した後のinstall tailをこのAPIへ接続できる。
本件は全folder scanner、全decoder、全fullscreen loadを新executorへ置き換える作業ではない。
新たに履歴/親要求を非同期化する部分のI/Oは上記workerへ置き、UI採用はメモリ上の結果を使う。

#### 9.3.2 履歴cursorとrollback snapshotを一つにする

履歴のcommitted vectorは既存のnormal/A/B ownerに残す。要求が持つ履歴proofは
**発行元workspaceの未変更cursorのread-only baseline**であり、復元命令ではない。
まず既存snapshotのactive workspace部分を再利用し、別history revision/rollback fieldを足さない。
要求のSourceProof variant、baselineとhead/entryを採用直前に検証する。
collection削除prune等がbaselineを変えたら要求をdiscardし、古いbaselineを書き戻さない。
このbaselineは履歴cursorの証明で、source Collectionのaccepted revisionを固定するsnapshotではない。
同ownerのPDF verification/Collection revision更新でcommitted cursorが変わらなければ、
surface依存要求をこの比較で失効させない。
仮cursorの`previous`照合もSourceProofの安定した意味へ投影し、表示hintを含む全restore entryの
一致をsurface variantへ重ねない。committed stackの変更検出とsource ownerの意味の検証を分ける。

連打で複数地点を選べた既存操作を失わないため、`SmartHistoryPeek`のoriginal/virtual cursorを
**一つのnavigation要求の履歴plan**へ統合する。連続Replayだけが同じcommitted baselineから
仮cursorを進め、前のworkerを取消して最新targetのworkerへplanをmoveする。
仮cursor/target routeにfilter値を保存しない。Smart→通常Pathで現行の`next.commit(self)`＋
rollbackに戻す経路（`smart_folder.rs:2971`）も、planを新しい読込phaseへmoveして未採用のまま渡す。
仮cursorがsourceへ戻ればworker/planをdropするだけで、facet・committed stackを動かさない。
通常open/親/slot切替という別intentが勝つ場合は古いplanを捨て、**表示中のcommitted source**を
新要求のsourceとする。未表示の中間targetをDirectの移動元にしない。
toolbarの連打可否/target表示はこのplanからread-onlyに投影できるが、採用検証はcommitted baselineを使う。
仮cursor自体は現行Smart履歴にも存在するため、二つのpeek ownerを併置しない。
単発Replayはcommitted headとtarget entryの一致を検証する。複数Replayの最終targetは
committed headそのものではないため、未変更baselineからplanをpureに再適用した仮cursor/targetとの
一致を検証する。共通採用後に旧helperで再び`head == final_target`を検査・popしてはならない。
成功時はplanの最終back/forwardを一回だけ確定する。途中targetの読込成功やfilter退避は不要で、
履歴entryの並びは既存peekのpop/push/dedup/MAX規則を使って算出する。

現行`FolderNavHistorySnapshot`全体を一括廃止するのではない。履歴要求については
`history_nav_rollback`、`ArchiveConvertState::nav_history_rollback`、
`EpubOpenRestore.history`の**同じ要求に対応するstack rollback**を共通値に置換する。
archive/EPUB dialogには共通要求へのcontinuationだけを残し、abort時のstack restoreをなくす。
検索自身のnav stack、address_before、retained PDF verifier、既存の非履歴restore用途はそのownerに残せる。
一つの要求が共通navigation値と旧history rollback snapshotの両方を保持する移行状態は認めない。
legacy snapshot restoreが残る用途も、facet ownerを復元せず、catalog prune契約を維持する。

`suppress_record_once`は、移行済み要求の履歴責務をinstall側の自動記録へ重複させるために使わない。
Direct/Replay等のenumが記録可否を決め、共通採用が一回だけ履歴を書く。
既存非移行callerの互換one-shotを残す場合はadapterで消費し、common採用の後へ漏らさない。

#### 9.3.3 単一の成功採用点

`adopt_main_list_navigation(navigation, prepared_install)`（提案名）を**新たに整備する**。
既存の`commit_physical_history_transition`、folder scan install、warm/cold PDF adoption、
typed surface/root install、ZIP prefix installはここへのadapterになる。
各load ownerの妥当性検証と既存のread-only context交換を混同しない。

1. mainのnavigation要求であること、request/context/cancel、SourceProof variant、
   baseline/planを検証。scope、Snapshot制限、宛先に必要なcatalog/revision、EPUB lease等の
   採用拒否を先に解決する。scan/列挙/row構築、ZIP collapse後prefixとlogical aliasの解決も先に済ませる。
   **sourceに既存sidecar hydrationが動いている場合のadmission gate**は維持する。
   **新しい宛先で始めるsidecarの成功は採用条件にしない**。両者を同じgateと呼ばない。
   `prepared_install`はsuccess payloadを持ち、ここで失敗なら要求をdropしてsourceを保つ。
2. sourceの選択/スクロールを既存保存経路へ渡す。成功payloadで確定した宛先entry/routeを使い、
   facet reducerを一回適用する。退出したsaved frameを復元・消費し、Directなら成功した親子edgeを
   作り、Replayなら保存routeを採用してlive親条件を新たに退避する。不要になった子frame/条件だけを
   捨てる。`place_keys`除去とname runtime同期を済ませる。
3. location/surface/BS provenance、itemsとその世代をinstallする。ここで宛先は**採用済み**。
   metadata/selectionのhydration用continuationを得るが、sidecarをまだ開始せずstep 4へ渡す。
   sidecar不要の経路も、この時点ではhistoryを書く別tailを呼ばない。
4. 同じUI-thread呼出し内で、Directの記録、Replayの仮cursor確定、Restore/SameLocationの無変更を
   実行して要求を消費する。MRU/active workspace targetの更新も既存のintent別規則で一回だけ行う。
5. **採用後のhydration**として既存`begin_sidecar_restore`を呼ぶ。sidecar不要なら既存resume tailへ
   直行する。sidecar完了後は採用済みfacetで最初のvisible/order/selectionを計算し、親selection anchor、
   §1.328の位置復元、StartupListIntentのfirst-display部分を従来の順で適用する。
   このtailはnavigation/facet/historyを再commitせず、metadataによる可視性更新だけを行う。

これはDB transactionや全bundleのcopyではなく、**拒否可能なprepareと拒否不能なinstall tailを分けるAPI**。
step 2後に`false`/early returnで採用を拒む関数を呼んではならない。現行の
`adopt_collection_surface_for_physical_load`や`start_loading_items_inner`等にあるscope/sidecar/leaseの
**sourceの既存sidecarに対する拒否**とscope/lease拒否はstep 1へ寄せ、成功を返す前に確定する。
現在は「loader呼出しがLoaded」だけでhistoryを進める
箇所もあるので、pending admissionとvisible adoptionを区別し、Loadedを採用済みの代用にしない。
表示を確定した後のthumbnail/metadata worker失敗は既存のitem-level errorとして扱い、履歴を巻き戻さない。
warm PDFの後続verificationは既存契約を維持し、再びfacet/historyをcommitしない。

実装境界は`start_loading_items_inner`の**items installまで**と、sidecar開始/first-display tailを分けること。
既存`SidecarLoadContinuation`（`sidecar_restore.rs:90`）は採用後のhydrationを唯一所有し、
共通navigation要求、pre-pop履歴snapshot、facet値/frameのcopyを持たない。
要求のselection/restore intentは採用時にこのcontinuationへmoveし、完了時の位置補正に使う。
`resume_loading_items_after_sidecar`と`finish_main_list_open`のfirst-display/起動保存部分は
既存タイミングを維持し、移動のhistory/facet採用と混同しない。
sidecarのcontext・新items generation・source path照合（`sidecar_restore.rs:1055`）は維持する。
surface依存のnavigationを許すことは、hydrateする行世代の照合を緩めることではない。

新itemsの採用を、補正/タグの未復元画像の先出しと解釈しない。sidecarのinput gate、
待機表示、first-display前のhydration、deferred fullscreenと旧表示unit保持は既存どおり。
sidecar開始不要/失敗は既存resume・warning経路で採用済み一覧を仕上げ、移動・facet・履歴を戻さない。
部分的にcommit済みのsidecar効果も既存の復元契約で保持する。context退役/generation不一致の
late hydrationはそのcontinuationだけをdiscardし、別contextや新移動をrollbackしない。
offscreen sidecar、別のnavigation rollback、sidecar完了までhistoryだけ未確定にする中間状態は追加しない。

#### 9.3.4 producer / consumer棚卸し（2026-10-08、製品codeのみ）

下表は入口名とowner境界の一覧。tests内の直接field操作は製品producerとして数えない。
実装時はfield/helper参照を再検索し、表の各群を共通要求/採用に接続したことを確認する。

| 履歴stackのproducer / consumer | 現行接続と改訂時の扱い |
| --- | --- |
| 作成・workspace選択・Settings投影 | `App`初期化、`QuickFolderWorkspace`、`active_quick_folder_workspace[_mut]`、`sync_quick_folder_settings`。normal/A/B vector owner、Settings保存path/MRUは維持。entry値だけroute付きにする |
| 直接移動の記録 | `push_active_folder_nav_back_stack`、`clear_active_folder_nav_forward_stack`、`push_folder_nav_stack`、`record_folder_nav_transition[_with_rating/_from_current/_from_restore]`、`push_nav_history_entry`。共通Direct採用の内部primitive/adapterに限定。same-place dedupとMAX_FOLDER_NAV_STACKを維持 |
| typed source記録 | `record_collection_nav_transition`、`record_collection_physical_nav_transition`（`collection_grid.rs`のroot/physical source commit）、`commit_rating_physical_load_owner`、`record_smart_folder_scope_transition`。sourceをsurface交換前にcaptureし、historyとfacetを一回の採用へ送る |
| synthetic entry | `enter_drive_list_from_navigation`、`enter_reading_history_from_menu`、`open_bookmark_browser`、Rating entry/restore、Collection open/parent、Smart install/root復帰。合成pathをFS親子判定せずtypedなprepared surfaceを採用。Collectionの既存loading shell採用時点は変更しない |
| targetの読出し/実行 | `folder_nav_current_target`、`folder_history_back_target/forward_target`、`dispatch_main_folder_history_input`、`dispatch_synthetic_folder_history_target[_with_rollback]`、`navigate_folder_history_back/forward`、`commit_staged_history_replay_after_adoption`。読出しはentry保持、先行popを廃止し成功時の一回のcursor commitへ統合 |
| 入力と親選択 | `ui_main.rs`のtoolbar/history menu/parent/cell、`handle_keyboard`と`update_frame`の入力merge、KeyAction GridHistoryBack/Forward、mouse/ring、`gamepad_input.rs::apply_folder_history_nav/handle_gamepad_grid_back`、`grid_parent_nav_target/resolve_grid_parent_nav/resolve_return_to_parent_nav/take_pending_return_to_parent_nav`。既存優先順位・keymapを維持しtyped intentだけを発行 |
| async requestのproof/commit | `PhysicalHistoryTransition`のstart/poll/source検証、`RatingNavigationTransition`のstart/poll/commit、`CollectionHistoryTransition`のstart/poll/source検証、`SmartFolderSourceLease`、`SmartHistoryPeek`のcapture/advance/is_current/commitと`advance_staged_smart_history`。既存phaseと要求別のSourceProofを維持し、共通navigation値がproof/planを単独所有 |
| classification/direct load・rollback | `open_direct_navigation_target[_classified]`、`poll_open_path_classification`、`load_folder_with_scan_*`、`adopt_collection_surface_for_physical_load`、PDF adoption、`folder_nav_history_snapshot/restore_folder_nav_history`、archive rollback attach/clearとdialog cancel/error、`EpubOpenRestore`。通常履歴のrollbackをread-only baselineへ置換。別要求が勝った後にsnapshotを書き戻さない |
| 検索等の透明origin・直接jump | `global_search_ui`のdrill/exit、favsearch/tagのnav open/restore、`context_menu::dismiss_source_for_jump_to_folder`、keyboard/context/grid openのsearch rollback、`collection_navigation::commit_collection_grid_source_open`。origin routeを既存return ownerからmove。jumpの先行`push_nav_history_entry`は成功Direct採用へ移す。自動再生/page continuationは従来どおり履歴を増やさない |
| 削除prune・read-only context | `collections::prune_collection_folder_history_from_ready_catalog`（normal/A/B、restore後にも適用）、viewer bundle fork/mount/swap/park/close、detachedのmain-history除外。prune後の旧要求を失効させる。context交換はglobal vector/facet ownerを変更しない |
| 採用後のsidecar hydration | `start_loading_items_inner`の新items install、`begin_sidecar_restore`、`sidecar_restore_context_current`、`poll_sidecar_restore`、`resume_loading_items_after_sidecar`。新世代を対象にfirst-displayを仕上げる。navigation/source proofとは別目的の既存ownerで、history/facetは変更しない |

| facet stashのproducer / consumer | 現行接続と改訂時の扱い |
| --- | --- |
| state作成 | `App`のscope/stack初期化、`FacetFilterSuppression`。一つの`FacetNavigationState`へ置換、ViewerContextBundleには入れない |
| rebuildの推定/場所条件除去 | `rebuild_visible_indices_impl`→`sync_facet_filter_scope`→`update_facet_filter_suppression_for_scope_change`、`clear_place_facet_for_scope_change`、`restore_facet_filter_suppression_for_path`。mutationを共通採用へ移す。rebuild/read-only再投影でscopeを推定してstashを動かさない |
| container/grid open | `suppress_current_facet_filter_at`、`maybe_suppress_facet_filter_for_opened_container[_path]`、`maybe_suppress_facet_filter_for_opened_zip_book`、`commit_grid_virtual_open_effects`、`commit_main_grid_archive_transition`、`resolve_main_folder_open_ready`、keyboard/ui_main/gamepadの各open。helperは要求adapterへ縮小、成功payload採用前にはfilterを変更しない |
| ZIP階層 | `zip_nav_show_current_level`、`maybe_restore_facet_filter_after_zip_level_change`。typed source archive＋実効prefixで同じreducerを使い、DB normalized keyのprefix推定をやめる |
| Smart root/child | `adopt_smart_child_session`と`install_prepared_smart_folder`のstash anchor書換え、`adopt_smart_child_ready`の退避、resident root復帰。anchorの後付けretargetを撤去し、成功したSmart ID/positionのrouteを採用 |
| Ctrl+G drill/return | `drill_into_container`、`drill_into_subfolder`、`drill_back_to_top`の全pop、`drill_back_one_level`のpath復元。検索origin/rootを含むrouteの成功採用へ接続し、main履歴は透明のまま |
| UI表示・手動復元・編集 | `facet_filter_suppressed`、`restore_facet_filter_suppression`、ui_mainの退避badge/各facet chip/menu/reset、`facet_name_filter.rs`のclear/edit/runtime同期。ownerの読出し/明示編集APIへ統合し、手動復元とIME動作を維持 |
| context再投影 | `rebuild_visible_indices_preserving_facet_scope`、`viewer_context_registry::swap_viewer_context_bundle`等。active Settings値・scope・frameを読み取り専用とし、同一main contextのnavigation成功だけがreducerを呼ぶ |

`rating_filter_suppressed_at`等の独立したrating filterは別機能のownerとして維持する。
facet修正のためにratingの意味や永続値を変更せず、同じ採用adapter内で既存効果の順序を保つ。

#### 9.3.5 open / switch / cancel / error / closeの契約

| lifecycle | history / facetへの効果 |
| --- | --- |
| open受理・分類・scan・列挙pending | source proofとtyped targetを一つの要求へcapture。要求自体はsourceの表示owner/current/active facet/stash/committed stackを書き換えない。committed warm PDF verificationは既存どおり履歴／分類pendingまたはdocument-open modal中は保留し、他の同owner行更新はSourceProofで継続可否を判断。address/loading表示は要求からのpreviewで、採用した現在地と混同しない |
| 成功 | §9.3.3を一回実行。同じZIPへの初回・history再入場・BS、Direct/CachedZip、warm/coldで結果が一致する |
| 別open/連続Replay・A/B切替 | 通常pendingは取消・置換できる。Replay連打は同じplanをmove、別intentは表示中sourceから作り直す。slot切替の成功時だけtarget slot/routeを採用。切替受理時に旧要求を退役させ、遅延replyは全SourceProofの共通switch epochとnative ownerで棄却。slot別stashは作らない |
| conversion/password待ち | 未採用の同じ要求phaseとして既存モーダルの操作受付規則を維持。成功payloadまでfacet/履歴を変更しない |
| sourceのsidecar待ち / destinationのsidecar hydration | sourceの既存hydration中は現行admission/input gateを維持。宛先ではstep 4までに移動・facet・履歴を採用してからsidecarを開始し、既存の待機表示/first-display/deferred fullscreenを仕上げる。sidecar結果で移動をrollbackしない |
| 採用前のcancel・scan/列挙エラー・worker disconnect・refusal・stale | 要求とそのworker/cancel/leaseだけを退役し、表示中owner・facet・committed cursorを保つ。同ownerの既存metadata更新をundoしない。既存toast等の通知を使い、履歴snapshotの書戻し、retry、delayによる救済をしない |
| reload・通知・ソート・fullscreenから同じ本のページ一覧 | SameLocationとしてroute/cursor不変、二重stashなし。表示位置や本内部prefixの既存復帰を維持 |
| sourceのPDF verification / Collection revision更新 | committed warm PDF verificationは履歴／分類pendingまたはdocument-open modal中は既存poll gateで保留する。保留対象外のSmart移動などで補正が進む場合、またはその他の同owner行更新では、surface依存要求は安定した意味が同じなら継続する。row/snapshot依存要求は既存generation/revision proofで再検証。同ID再open等の別ownerへは継続させない |
| Main context退役・park・detached fork/mount/swap/close | 要求がsource contextと共に移る場合もmain採用資格を満たす時だけcommit。一時mount/read-only swapは採用ではない。sourceが退役した要求はdropし、sibling/global ownerのrollbackをしない。既存detached predicate/viewportは変更しない |
| Collection削除prune | authoritative Ready catalogで既存どおりentryをprune。旧baselineの要求は失効。表示中childのPathへの既存投影を保ち、消えたCollectionへ復帰・再記録しない。次の実navigationでfacet frameを通常の退出規則で消費する |

§1.345のファイル種類設定・保存形式とは独立して実装する。新しいhistory/filter DB、paint内I/O、
UI threadのscanや待機、detached所有範囲の変更は含めない。

### 9.4 履歴再入場の設計既定（D14、旧Q14）

通常openと同じくlive親条件を退避し、子は条件なしから開始することを決定した既定とする。
子/履歴地点別の過去filter記憶は追加しない。利用者がその追加を要望した場合にだけ再相談する。

### 9.5 実装規模・残せる部分・公開済み動作の回帰検証

通常履歴移行は小さな退避helper修正ではない。見積りは**L、実装3〜5開発日＋検証/レビュー修正1〜2日**。
製品差分約1,000〜2,000行、tests約600〜1,200行、主な変更は`app.rs`、新facet reducer module、
history/typed restore、Smart、PDF/変換continuation、ui_main/global_search/gamepadのadapter。
今回の2指摘への対応もこのLの概算内とし、日数/行数の見積りは維持する。
改修範囲へsidecarのinstall/hydration接続分離とSourceProof variantを明示的に含める。
sidecarをoffscreen化しないため復元engine自体の全面移行は不要で、要求別validatorも既存を再利用する。
既存field/helper参照が広いための概算で、実測ではない。個別symptom patchを先に公開せず、
次の順で一つの§1.339 chunkを完成させる。

1. 報告された本番handler経路をredにし、pure route/reducer、route付きentryを整備する。
2. 通常履歴/親要求とrollback移行、既存typed ownerへのmove、拒否可能prepare/採用tailを整備する。
3. すべてのstash producer/consumerと入力adapterを接続し、横断回帰と指定gateを通す。

残せる部分: workerのscan/列挙アルゴリズム、cache/lease service、thumbnail/decoder/GPU経路、
ZIP treeの構築、Collection actor/prepare、Smart resident payload、PDF warm placeholder/verification、
変換/password modal、keymap/IME helpers、Settings/DB key/転送/起動復元の保存形式、
§1.328の位置保存/復元primitive、normal/A/Bの履歴vector owner、detached bundle交換/viewport。
これらの採用adapterやhistory entryの型が変わることはあるが、機能自体を再設計しない。
sidecar import/quiescence/cache復元、input gate/holdoverも残せる。
SourceProofのnative validatorを再利用し、共通のitems/revision guardで上書きしない。

公開済み動作へのリスクと必須回帰:

- **proof規則とswitch epoch**: Quick Folderの同slot再選択／同target A→B→Aに分類reply待ちを交差させ、
  旧要求の取消と旧証明の失効の両方を検証。実ペインEnter→scan待ち→同queryの実検索worker更新では採用を維持。
  コピー済み履歴宛先・RequiredFullscreen scanと同surface内のrow publicationの交差、query re-entryでの退役、SmartGrid等の行依存失効も検証する。
  Search prepareのRowは送信前captureから保持し、採用直前に取り直さない。同じreader再選択では
  採用済みBookmark AwaitingPage/帰路を保持し、未採用Resolvingだけを退役する。

- **cursor記録時期/連打**: 通常folder←/→、toolbar、Alt+左右、mouse、ring/gamepadが同じhandlerに着地する。
  prescan/workerの双方で成功後だけcursor変更、同place dedup/MAX上限/forward消去、BSはDirect、
  ZIP内部BSは外側cursor不変を検証。待ち中の←←/←→、Smart→通常folderへの連打handoffを
  controlled channelで試し、古いreply・旧snapshotが最新plan/表示を巻き戻さないことを検査する。
- **退避の帰路**: 報告の★3→ZIP→BS→←→→と、同じZIPを別parent/provenanceから開く対照をhandlerで検証。
  再入場はlive親条件、子編集は親へ漏れず、stash depthが増えない。drive/UNC/trailing separator/
  cross-drive、ZIP prefix/nested/collapse、PDF/EPUB/変換cache aliasをpure reducer＋採用handlerで検査する。
- **手動UI/入力**: badge手動復元後のrebuild/親復帰、parent条件編集、name-query runtime、place_keys、
  facet resetを検証。既存IME/KeyActionを変更しない。表示を変えた場合はheadless ui_snapshotを更新する。
- **採用の成否/位置**: scan/error/cancel/stale/snapshot拒否/source sidecar admission gate/EPUB lease拒否の直前・直後を
  handler/lifecycle testで検証。未採用時はsettings active値・frame・history・visible source不変。
  warm PDFの初回表示を遅らせず、cold/変換/passwordは同じ要求で継続し、後続verificationで二重commitしない。
  §1.328の選択/scroll、起動一覧復元、rating sort、search close/jump/drillの既存回帰を併用する。
- **採用後hydration**: 本番のitems install→sidecar開始→完了poll/resumeをcontrolled channelで検査する。
  hydration待ちの時点で宛先とroute、facet、historyが一回だけ採用されていること、first-displayは
  復元済みmetadataで計算されること、warning/resumeや完了で再push/popしないことを検証する。
  タグ/補正反映で可視性が変わる例、親選択/scroll、StartupListIntent、deferred fullscreen/旧表示unit保持を
  既存sidecar回帰と併用。late reply/context退役でも別contextのhistory/facetを戻さない。
- **証明の交差**: ★一覧→ZIP→cached PDF→Backでは、準備中の実PDF pollが補正を保留し、
  ForwardでBackを取消した後に補正が再開することを検査する。guardを外して交差を捏造しない。
  ★一覧→ZIP→別ZIP→Back／BSでは、毎frameの実pin通知consumerがZIPの同階層を再構築して
  items世代を進めてもtyped restoreが一回だけ採用され、同じ更新で明示openは失効する対照を入れる。
  ★一覧→ZIP→Collection→Backでは、実Collection actorの登録追加→revision publish→grid再installを
  ZIP準備待ちに交差させる。source items世代と地点のrevision／viewport hintが変わっても、同一ownerの
  typed Rating restoreは採用される。元共通Surface証明による意味identity／epoch検証は維持する。
  QuickFolderSwitch native証明でも同じ行更新を許し、switch epoch／宛先変更は拒否する。
  PDF placeholderのverification完了でitems世代が進む間に保留対象外のSmart移動をpendingにし、
  同じsurfaceなら有効で、別path/surfaceへのopenなら失効することを採用handlerで検査する。
  `staged_smart_folder_collection_lease_survives_revision_refresh`（`tests.rs:87301`）と
  `staged_smart_folder_collection_lease_rejects_same_id_reopen`を維持し、pending中にCollectionの行/revisionが
  更新される経路でも検査する。同じ更新がrow-boundなRating/Collection openを失効させる対照も入れる。
  各交差でhistory/route/stashの一回採用とsource保持を確認し、単にgenerationを無視するtestにしない。
- **workspace/context/削除**: normal/A/B別履歴、A→B→A後の古いreply、Ready catalog prune、
  detached/parked fork/mount/swap/close時のglobal facet/history不変を検証。slot別facet stashのtestは作らない。

実装時のgateはfocused（有効なred→green）、`cargo fmt`、`cargo test -p mimageviewer --lib`、
normal/portable core check、glyph lint、UIを変更した場合のui_snapshot、`scripts/build-dev.ps1`。
製品起動はしない。design-only改訂ではRust gate/buildは不要。
利用者から見た仕様変更は§1.339の誤適用解消と、失敗した移動で履歴/退避条件が先行しないことだけ。
追加modal、操作禁止、履歴地点別filter記憶、履歴連打の削除は採らない。
実装中に既存挙動を残せないと分かった場合は、具体例と費用を示して利用者へ再相談する。
現時点で新しい利用者質問はない。D14その他の2026-10-08確定仕様を変更しない。
sidecarの採用後hydrationとsource証明の区別は現行閲覧契約を維持する内部設計の訂正で、
復元前の画像を先に見せる、Smart移動を取消しやすくする等の利用者動作変更を加えない。
再レビュー2指摘への改訂は設計のみで、独立reviewerの構造承認保留が解消したとは主張しない。

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
