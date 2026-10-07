# 表示するファイル種類 — §1.345 / 履歴再入場のfacet退避 — §1.339

作成: 2026-10-07、ラインA。状態: **設計案、未実装、利用者の回答と設計担当・独立レビュー待ち**。
本書の新しい型・APIは提案であり、現行コードに存在するという主張ではない。
実アプリは起動していない。§1.339の観測者は利用者、原因の根拠は下記のコード調査。

## 1. 決定済みの範囲と今回の境界

[次版バックログ](next-release-backlog.md)の「次の版の決定 (利用者 2026-10-07)」を正本とする。

- §1.345は既存facetの継承拡張をやめ、mIV全体の「表示するファイル種類」設定に作り直す。
  一覧生成の最初の層で除く。同名処理・AppleDouble除去と同じ生成段に置き、退避・復元を持たない。
- §1.339は既存facet退避の不具合として独立して直す。§1.345の実装・設定ONを前提にしない。
- 本書は設計のみ。適用範囲・本判定・操作導線などの利用者判断は§8の質問に残す。
  推奨案を確定仕様として実装しない。
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
個別除外では実際のextensionを使う。folder/driveは常に通行可能な構造として保持する案（Q2）。
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
論理元文書のPDF/EPUB categoryを使う案（Q3）。ZipDirは通路として保持し、内部にある
archive member本体は実memberの形式で判定する。仮想prefixを実ファイルextensionと誤認しない。
missing collection/bookmark参照はlast-known kindと論理元pathで判定し、
型を推定できない参照は保持する案。選別にstatを足さない。

生成順は、対応形式認識・OS/system/AppleDouble除去 → **本ポリシー** →
同名優先・重複除去 → 本判定/stack/集約/ソート/代表選定 → aligned install →
現在地のfacet/名前/評価絞り込み。新設定とfacetはANDであり、facetから除外項目を復活させない。
例: 動画を除外したら同名JPEGを動画の存在で消さず、ZIPを除外したら同名RARをZIPの存在で消さない（Q4）。
同名判定用にdirectoryの全候補が必要でも、勝者候補集合は本ポリシー通過後にする。

## 4. 全producerの接続表

各行の入口は現行コード。接続位置とownerは提案。すべて§3の`FileTypeVisibility`へ通す。
表示rowを除く場合は同じindexでmeta・binding・stable key・thumbnail sourceも除き、
producer側で同じ入力列を保ったまま結果を組み立てる。install後のVecだけをretainしない。

| 一覧producer | 共通ownerを通す位置・注意 |
| --- | --- |
| 通常folder / 製本folder / 本棚 | `folder_scan.rs::scan_directory_entries`のrecognition後、`materialize_local_folder_listing_with_order`の同名処理前。manifestページは元ページ形式で判定し、製本データを編集しない |
| ZIP/CBZ内部 | `zip_loader.rs`の画像entry認識後、`zip_tree.rs::materialize_level`がページ/代表/ZipDirを作る前。生treeを設定別に破壊せず、同じpolicyで表示projectionを作る |
| PDF/EPUB内部 | `app.rs`のPDF prepared enumerationからページitemsを作る前。logical pathの文書categoryを使う。PDF page_num、EPUB generation/leaseは維持 |
| converted archive / direct RAR | 外側tileは元形式。`ConvertedArchiveSourceState::load_path`と`archive_source_override`はI/O用identityで、表示分類の元形式を保持。内部は上のZIP tree経路へ統合 |
| rating list ★1〜5 | `rating_view.rs::prepare_rating_view` / `sort_and_materialize_rows`のrows→items前。source/meta/★時刻を同時に投影し、rating.db行は残す |
| 検索 Ctrl+S / Ctrl+G / Ctrl+F | `app.rs::apply_favsearch_results`、`global_search_ui.rs`のstream結果→flat/SearchContainer集約前。除外hitをhit_count・代表に含めない。Ctrl+Fは生成済み基礎一覧を対象にするので新しい別判定を足さない |
| tag view | `tag_view.rs`のDB結果→行、`app.rs::apply_tag_view_result`の集約前。タグ索引/DBは削除しない |
| collection root / physical child | `collection_grid.rs::prepare_collection_grid_install`でsource resolution後、表示binding/thumbnail source/reader order前。`collection_store/prepare.rs`のexport用full snapshotに表示除外を入れない。子はfolder/virtual共通経路 |
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
有効候補の自動代表（なければアイコン）へ表示だけ切り替える案（Q5）。
動画のsidecar画像はメタデータ資源なので、画像を一覧から除外しても表示中の動画のsidecarには使う案。
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
除外archiveを候補に入れず、通過する直接mediaがないfolderはskip対象にする案（Q6）。
子folderがあるだけで走査を打ち切らず、従来DFSを継続する。直接クリックは空folderにも入れる。
skip_limitを使い切った通常gridの既存fallbackと、fullscreenの境界停止は保存する。
「絶対に空folderへ止まらない」ために無制限DFSを追加しない。

推奨の画像本判定は「ポリシーと同名処理後の、表示対象となる認識mediaが非空で全て画像、
表示対象の子コンテナなし」。画像+非表示動画なら画像本になる（Q7）。
Folder構造は保持するので実子folderがあれば従来どおり非本。全画像除外は非本。
通常openの`scanned_folder_is_image_book`、page-count worker、sub展開の畳み込み、
Remote読順を同じ投影と述語に揃える。`image_page_recognition_fingerprint`もpolicy値を含める。
既存resume raw index/keyはこの設計で移行しない。画像ページ列が変わる場合は、現行復元検証で
有効なものだけ採用し、保存済みmeter比率は次の読書記録まで維持する。
非表示pageを必須targetとしたbookmark/検索復元は先頭へのsilent fallbackをせず、理由を通知する案（Q3）。

### 5.3 件数・badge・facet・metadata

一覧件数、表示/選択/check、検索hit_count、stack count、Smart結果数、
本としての有効ページ数は通過後の同じ列を参照する。総登録数はcollection/bookmark等のDB全件数として
区別し、「表示X / 登録Y」などで隠した登録が消えたと誤認させない。
folderの`OmittedFolderEntryCounts`に種類設定の内訳を追加する案（Q8）。
いまあるhidden/same_name/unsupportedと二重計上しない。badge描画でFSを再計数しない。
facetの候補・値別件数は基礎一覧の通過後だけを母集団にする。

表示設定はデータ削除ではない。tags.db、rating.db、bookmarks、collection手動順/ID、
reading_history、sidecar、編集・pin、索引のingest/export/backup/rename migrationを変更しない。
media用JSON/TXT/XMP、`mimageviewer.dat`、manifest等は一覧対象形式とは別の補助入力として
従来どおり読む。隠したファイルのtagを消したり、metadata cleanupのmissing根拠にしたりしない。
collection手動並べ替えはvisible subsetのstable IDだけを並べ替え、隠れたentryを削除しない。
既存full-entry順へのmerge ownerを使う（実装前にhidden-entryの保持をテストする）。

## 6. 永続化とUI案

新fieldは`Settings.file_type_visibility`のみ。既定は全対応形式を表示。
settings.dbの既存Settings carrierへ追加し、旧fieldの意味・既存facet値を移行しない。
新設定は未出荷、旧設定ファイルからのmissing fieldはdefaultで読み込む。
古い版へ戻したときの未知field保存/互換判定はSettings DBの現行方針を使い、
schema/tableを別途新設する理由はない。enum未知値を空集合へ黙って変換しない。

`settings_transfer.rs::preferences_policy!`の**export対象**へ「表示するファイル種類」として
明示分類する案（Q9）。パス・ユーザーデータを含まない全体環境設定なので別PCへ転送可能。
validated parse、default/roundtrip、全field分類、import失敗で元値不変、
preferences OK/Cancel、backup/recoveryの境界を検査する。
favorite_view_overlay、A/B記憶、起動一覧recordには追加しない。

環境設定「ファイル処理」に独立項目を置く案。
category checkbox＋拡張子詳細、全表示へ戻すボタン、除外適用の説明を示す。
既存「書庫を無視/確認/変換」とは別設定で、表示を許可しても開封処理が許可されるとは限らない。
toolbar quick導線は同じ設定を開く任意登録ボタンと適用中の印を推奨し、
一時解除boolや第二のeffective policyを増やさない（Q10）。
「全部表示」操作も採用するなら同じ永続設定の変更として扱い、元状態stashを作らない。
新キー操作を付ける場合は`KeyAction`とkeymap helper一式、既定割当なし。raw keyイベントを追加しない。

Remoteは同じ設定を本体から読む。端末側に設定編集APIを増やさない案。
各既存prepared/read cache identityにpolicy値を含め、次のlist/read要求で最新設定と照合する。
payload説明を増やすなら`crates/remote-ipc`のprotocol更新を同時に行う。

## 7. 状態の組み合わせを減らす検討

**採用を推奨**: 設定のOKで一回確定、既存reload/prepareで基礎一覧を作り直す。
prefs draftと表示policyを混ぜず、Cancelなら何も変えない。
同名処理/書庫処理変更の既存reload経路を調べて再利用し、live retain用の専用state machineを作らない。
検索・sub展開・snapshotは既存close→元場所reloadを使う案（Q11）。
閉じると検索結果/一時snapshotが失われるため利用者の回答前に採用しない。
collection/Smartは保存定義から既存再prepareできるのでその経路を使う。

設定変更中の各detached viewerを閉じて作り直す案も検討した。
動画再生や開いている本を中断する既存挙動の削減になるため無承認では採用しない。
推奨は開いているviewerの読書/再生contextと読順をその終了まで保持し、
次の一覧生成/open/reloadから新policyを使う案（Q12）。
新policyを適用した一覧へ戻る境界では除外済みのselected/checkを持ち越さない。
この例外は「すべての新しい一覧生成には同policy」を維持し、開いているsessionの寿命だけを区切る。
即時に全windowを更新する要求なら、既存context採用/worker cancel/cache所有境界を設計レビューしてから
別chunkで実装する。mounted以外のcontextをmain経由で一括resetしない。
detached述語/viewport変更に達した場合はrework§2の合意と§11記録が必要。

長い変換・password・保存は既存モーダルを維持し、その途中でprefs適用を許可しない。
追加のrollback・supersession・resume pendingは作らない。
rare cache/DB failureの多段回復は追加しない。現行ログ/通知/次回再生成の範囲で扱い、
利用者の設定や登録データを落とす割り切りはしない。

## 8. 利用者への質問（未回答）

| ID | 質問 | 推奨回答と影響 |
| --- | --- | --- |
| Q1 | Remote端末にも、この本体共通設定を編集するUI/APIを追加しますか？ | 今回は追加しない。全一覧・Remoteへ同じ設定を適用すること自体は決定済み。本体の環境設定で変更する |
| Q2 | フォルダ/ドライブ自体も種類設定で隠しますか？ | いいえ。移動する通路を残し、ファイル形式だけを対象にする |
| Q3 | 本の内部にも適用し、除外したページへの明示openは理由を出して拒否しますか？ | はい。ZIP画像はmember形式、PDF/EPUBページは元文書形式。本の直接path指定でも生成するページ一覧には適用する。外部からの単体画像/動画/音声openは維持し、その兄弟一覧には適用する。アクセス権限の代わりにはしない |
| Q4 | 同名優先は表示対象候補だけで判定しますか？ | はい。動画を隠すと同名画像を、ZIPを隠すと同名RARを表示できる。現在の通常利用との違いは設定を使った場合だけ |
| Q5 | 除外形式をpinしたfolder代表はどうしますか？ | pin登録を保存したまま、表示は自動代表/アイコン。video sidecar等の補助画像は除外しない |
| Q6 | Ctrl+上下は表示対象mediaがないfolderをskipしますか？ | はい。従来skip_limit/fallbackは維持。直接クリックでは空folderにも入れる |
| Q7 | 動画等を非表示にした混在folderも「画像だけの本」と判定しますか？ | はい。表示mediaが非空で全画像、子コンテナなしという共通判定。自動open・page count・sub展開・Remoteを揃える |
| Q8 | 隠した件数を既存「表示していない項目」の内訳に含めますか？ | はい。「ファイル種類の設定」を別理由として数える。登録数と表示数も区別 |
| Q9 | この設定を環境設定の書き出し/取り込み対象にしますか？ | はい。既定全表示、新fieldのみ。facet・favorite・A/Bへコピーしない |
| Q10 | 切替導線は環境設定＋任意toolbar設定ボタンでよいですか？ | はい。独立した一時解除toggleは作らない。必要なら同じ設定を全表示へ戻す操作を用意 |
| Q11 | 適用時に検索/sub展開/snapshotを閉じて元場所を再読込してよいですか？ | はいを推奨。専用のlive再構築を減らせるが一時結果が閉じる。これが不便なら既存各producerの再実行を設計する |
| Q12 | 開いている読書・再生sessionは保持し、次の一覧生成から適用してよいですか？ | はいを推奨。全window即時適用による本/動画中断を避ける。即時適用が必要なら費用と中断動作を再相談 |
| Q13 | 分類単位と細かい除外は§3のcategory＋拡張子でよいですか？ | はい。画像とRAW、PDFとEPUB、ZIPとRAR/7z/LZHを分け、必要な拡張子だけの除外も可能にする。未対応形式の対応追加にはしない |

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
`FacetNavigationState`へ集約する。active値の永続carrierは既存`settings.facet_filter`を保持し、
ownerはその値を受け渡す唯一のmutation APIを持つ。別active filter copyを作らない。
新しい`pending_facet_*` / bool / sentinelをAppへ足さない。

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

1. 既存ownerがsource context / items generation / slot / request / history headの妥当性と
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
| toolbar/キー/マウスの←→、BS、アドレス、A/B | 既存routerを保ち、成功した同scope採用を一つのreducerへ送る。A/Bのstashは他slotと混ぜない |
| 本内ZIP階層/入れ子/単一wrapper collapse、fullscreen→ページ一覧 | prefix scopeを使い、同じscopeでは何もしない。退出したbook分だけ復元 |
| reload/notify/ソート/同場所再install | route不変なので二重stashを作らない |
| conversion/password/sidecar待ち、cancel/error/superseded、close | 未採用要求のdiscardだけ。既存モーダル/取消ownerを維持。成功までactive値とstashに触れない |
| F12/複数viewer | 既存context runtimeのfacet所有範囲に閉じる。main historyやsibling contextを変更しない。capture/restore bundleの持ち運びを監査し、detached経路を触るならrework合意を先に取る |

簡素化の検討: 履歴の→を禁止する、facetを常時解除する、ZIPを開くたび全stashをclearする案は
既存機能を削るため不採用。全場所のfilter snapshot永続化も要求外で、親編集との古い値競合を増やすので不採用。
既存のモーダルを維持して変換中の割込みを増やさず、既存採用transactionと親chainを再利用する。
新ownerは既存runtime fieldsを置換するもので、追加の並行状態ではない。
§1.345とは設定・owner・受入テストを分離し、1.339単独の差分として先に実装できる。

### 9.4 利用者への追加質問（未回答）

Q14: 履歴で本へ入り直したときは、親のその時点の条件を退避し、本内は条件なしから始める
現行open仕様に揃えてよいですか？ **推奨: はい**。子ごとの過去filter記憶は追加しない。
回答で履歴地点ごとのfilter記憶が必要になった場合は別仕様として再設計する。

## 10. 受入条件・実装順・引継ぎ

§1.339を独立chunkにし、Q14の判断と設計owner/独立reviewerの構造合意後に実装する。
まずreported routeを本番history handlerでredにし、zip内jpgが空にならないこと、
戻ると★3のzip条件が復元すること、繰り返してもstash深さが増えないことを検査する。
source suffixへのguardだけでは通らない同じZIPへの別親provenanceの対照を入れる。
drive/UNC root、末尾区切り、cross-drive、back再入場、PDF/EPUB/convertedのlogical alias、
ZIP nested prefix、folder reload、親条件編集、name-query/placeKeys、取消/失敗/stale、
A/Bとsibling context不変を純粋reducer＋採用handlerで検査する。
§1.328選択・可視性、起動復元、rating sort、search退出の既存回帰を併用する。

§1.345はQ1〜13の判断後、分類/Settings転送 → folder/virtualと派生predicate →
aggregate producer → Remoteとcache/reloadの順でcoherent chunkに分ける。
部分producerだけを公開して「app全体対応」としない。未接続があれば内部実装段階のまま引き継ぐ。
必要なテスト:

- all-visible default、category/extension/RAW/EPUB/仮想member分類、同名競合順、AppleDoubleとの内訳。
- §4各producerの同fixture投影、aligned meta/binding/ID、stack/検索countsと代表が除外済みになること。
- image+video非表示の本判定、全部画像除外、子folder、Ctrl+上下skip/fallback、folder pin/auto/catalog再利用。
- Settings旧値/default/roundtrip/export-import/Cancel、policy変更後のstale完了・reload・snapshot復帰。
- data rowsとsidecar/tag/collection順が不変、local/Remoteの表示列・読順・count一致、Remote prepared cache更新。
- headless UI snapshot（prefs・適用中表示・件数）、full lib、fmt、通常/portable core check、glyph。

実機確認はcoordinatorが具体的なシナリオ・時間・入力/使い捨てdataの範囲を提示し、
利用者の明示承認を得る検証枠へ回す。製品バイナリをこのworktreeの実装担当は起動しない。
本書には独立review済みの主張を含めない。実装担当のコード前提照合は独立reviewの代わりではない。
