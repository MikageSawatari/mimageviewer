# §1.242 本の構成 — 設計案

更新: 2026-10-10、R3。ライン G / `next-book`、調査基準 `14fe0e319` (設計以外のソースは `463187691` と同じ)。
状態: R2再レビュー REVISE (P2×2 / P3×1、初回P1は解消との評価) を受けた改訂案。
利用者回答、設計採択、R3独立再レビューは未実施。
本書の型名・新規ファイル名は提案であり、現行実装を表さない。実行時の観測はなく、以下の現状はソース調査による。

## 1. 要件と範囲

正本は [next-release-backlog.md §1.242 / §1.364](next-release-backlog.md)。2026-09-15 の利用者決定と
2026-10-10 の追補を区別する。

- ZIP/CBZ、PDF、画像の本フォルダ、直接閲覧・変換閲覧する対応書庫に、本単位の非破壊な構成を保存する。
  書庫階層ごとに独立する。製本フォルダは構成作成対象外。変換書庫の保存先 identity は元書庫。
- ページの並べ替え、非表示、任意位置の白紙、見開きの組、単ページ配置を編集する。
  表紙あり/なし・右開き/左開きを構成に含め、通常の全体/本別自動設定より優先する。
- 作成開始時の自動構成を初期値に取り込む。その後、全体設定やページ寸法の後着で手動の組を作り直さない。
  一覧も構成順とし、非表示ページは通常一覧に出さない。未登録の実ページは末尾に足す。
- 非表示ページへのしおり、検索、評価一覧、保存位置等からの移動を拒否してトーストを出す。
  ファイル自体、ページ編集、しおり等の記録は消さない。「非表示」を実ファイルの「削除」と混同しない。
- 白紙は構成だけの要素。一覧・単ページ表示・実ページ数・書き出し・製本追加には含めない。
  横長分割は本の構成に保存せず、閲覧方法として独立させる。
- **2026-10-10 決定**: 構成がある本の見開きでは一時ずらしを使わない。
  Ctrl+←/→、カスタム割り当て、ゲームパッド等の同じ操作を共通 owner で拒否する。
  §1.364 の保存/解除 UI は構成なしの本だけ。単ページ時の従来のページ移動は維持する。

EPUB→PDF は現行共通経路を壊さないが、EPUBへの構成作成を今回の対応対象に追加するかは未決定 (§14)。
写真の通常フォルダ、スタック、検索/コレクションの結果順を本順に変更する機能ではない。
ただし、それらから本のページを開く入口でも非表示の判定を迂回させない。
集約/混在の具体的表示、Remote取得時のdraft、方向/表紙小編集、画像folderの外部移動は
今回のレビューで明確化した**採択待ちの推奨案**。§14の回答を既存の利用者決定と混同しない。

参照した原則: `CLAUDE.md` のバグ修正・状態削減・まれな失敗・退行相談・永続データ・IME・UI文字列、
[detached 憲法 §2](detached-rework-plan.md#2-憲法-全ステージ共通の不変条件禁止事項-最重要)、
[architecture-overview.md](architecture-overview.md)、[virtual-folders.md](virtual-folders.md)、
[display-pipeline.md](display-pipeline.md)、[本の読み順の旧計画](archive/folders-archives/book-page-order-plan.md)、
[§1.239/§1.240 計画](spread-endpoint-and-blank-pages-plan.md)、[web-remote-plan.md](web-remote-plan.md)、
[keymap-spec.md](keymap-spec.md)、[key-customization-impl-plan.md](key-customization-impl-plan.md)。
旧計画の「未実装」や行番号を現状の証拠として流用しない。

## 2. バックログの前提を現行コードで照合した結果

行番号は上記 HEAD のもの。実装開始時はライン E 統合後の symbol で再照合する。
「成立」はコード構造の確認であり、実機での成功を意味しない。

| 9月時点の前提 | 現行の根拠 (file:line) | 判定・設計への反映 |
| --- | --- | --- |
| 本は名前順で items を作る | `src/app.rs:42` (`BOOK_READING_PAGE_ORDER`)、`:418` (`folder_media_sort_order`)、`src/app/folder_scan.rs:276`、`src/app.rs:35212` (`materialize_zip_grid`) | 成立。PDFは番号順、製本は Numeric、通常フォルダは一覧設定。旧読み順計画の未実装記載は古い |
| items 順を変えれば下流が揃う | `src/ui_fullscreen.rs:16022` (`ViewerNavigationCaches`)、`src/app.rs:56783` (`current_reading_history_page_position`)、`src/ui_fullscreen.rs:45263` (`grid_selection_indices`) | 主要な下流は追随するが、それだけで完了しない。別 producer と直接 leaf open を揃える必要がある |
| `visible_indices` は昇順 | `src/app.rs:488`、`:69000` (`binary_search`) | 成立。構成を visible_indices の任意並べ替えとして実装しない |
| 末尾補助等の適格性は nav数=items数 | `src/ui_fullscreen.rs:16845` (`spread_complete_page_permutation`) | **更新が必要**。現行は `smart_folder_rule_total()` と資格・page data・重複なしの証明。`items.len()` 比較だけではない。構成用の complete proof を生成境界で渡す |
| ZIPの列生成が分散 | `src/app.rs:33158`, `:35197`, `:35212`, `:35253`, `:35608`, `:35970`, `:36002` | 成立。初回、物理履歴・評価ZipDir着地、階層移動、DFS、スライドショーの本跨ぎを共通化する |
| 画像フォルダとPDFにも個別列生成 | `src/app/folder_scan.rs:257`, `:276`, `:421`、`src/app.rs:36433` (`adopt_direct_pdf_rows`)、`:37319` (`poll_pdf_enumerate`) | 成立。走査/列挙のI/Oを一本にするのではなく、列挙後の本投影 owner を一本にする |
| しおり/スナップショットが階層列を再生成 | `src/book_bookmarks.rs:1525` (`resolve_archive_bookmark_target`)、`src/app.rs:47249`、`src/app/snapshot_ops.rs:2241` | 成立。しおり resolver は raw index を算出する。構成後 index と混同できない |
| Remote ZIPは arrange を通らない | `src/remote_ipc/container.rs:3963`, `:4024` (`enumerate_zip_page_context`)、`:5670`, `:5729` (`enumerate_zip`) | **入口ごとに異なる**。page context側は arrange済み、通常列挙は materialize直後。差が出る実行観測は未確認。二つとも共通投影へ置換 |
| Remote画像フォルダも独自 | `src/remote_ipc/container.rs:5552`, `:5565`、`src/app/folder_scan.rs:257` | 既に production folder listing を再利用。新しいRemote専用folder sorterを作らない |
| 編集中は items を触らず保存後 reload | `src/app.rs:35331`, `:36450` (`prepare_visible_install`)、`src/app/viewer_context_registry.rs:1799` (`set_items_generation`) | 実装可能な所有境界。現在のindexからの直接並べ替えは不可 |
| 白紙を GridItem にしない | `src/ui_fullscreen.rs:9433` (`SpreadPair`)、`:14174` (`SpreadPageOccurrence`)、`:14338` (`compose_spread_navigation_unit`) | 成立。idxは実在ページ。§1.240の白い相方はplacementで表現済み。新規偽パス/texture/sourceを作る理由がない |
| 見開きの編集は左ページへpivot | `src/app.rs:11760` (`plan_page_edit_pivot`) | 成立。白紙をDoubleの片側にすると編集targetが壊れる。白紙相方はSingleへ投影 |
| 見開きは両textureが必要 | `src/ui_fullscreen.rs:40808`, `:40903` (左右source選択・描画)、`src/materializer.rs:44` (`MergedSpread`) | 成立。完成/通過画像はunit単位で揃える。白紙にsource readinessを要求しない |
| builderは隣接/横長/shiftのみ | `src/ui_fullscreen.rs:14769`, `:14786` | **追加あり**。現在は `PageAloneSettings` と非pairable境界・forced singleton原因も入力。既存自動builderを構成に再適用しない |
| unit/nav cacheに構成がない | `src/ui_fullscreen.rs:14447` (`SpreadDisplayUnitsToken`)、`:16043` (`ensure_source`)、`:16766` | 成立。unit tokenはitems世代/mode/shift/landscape epoch/page_alone/nav、nav sourceは世代/flow。構成revisionと列投影を同一snapshotに結ぶ |
| resumeはitems indexだけ | `src/book_resume_db.rs:35`, `:78`, `:140`, `:174`, `:437`、`src/app/file_type_resume.rs:15`, `:135`、`src/app.rs:56644`, `:56871` | **古い**。raw index + optional logical identityのtransaction保存・worker構築対応表が既にある。既存型に構成を投影し、raw-only旧記録だけ橋渡しする |
| 履歴の位置も要確認 | `src/reading_history_db.rs:59`、`src/app.rs:22179`, `:22225`, `:56783` | historyのlast_pageは1-based読書ordinal、identityではない。履歴はcontainer openを経由する。位置の正本はresumeへ寄せ、表示ordinalを再開indexに転用しない |
| rename/purge/export/content restore登録が必要 | `src/rename_key_migration.rs:939`, `:1198`, `:1299`, `:1447`, `:1523`、`src/content_identity/restore.rs:181`, `:414` | 成立。行キー移行だけではJSON内の画像相対identity改名は更新されない。generic copyと専用ページ改名の双方が必要 |
| metadata transferを版上げ | `src/metadata_transfer.rs:27`, `:453`, `:492`, `:3203`、`src/app/metadata_import_refresh.rs:534` | 現行出力v9、入力v7/v8/v9。v7だけからの更新案は古い。既存入力版を保持し、新版は統合時の最新+1 |
| F12 bundle登録 | `src/app/viewer_context_registry.rs:809`, `:815`, `:898`, `:1061`, `:2226`, `:2500`, `:3451` | 成立。items、identity index、unit cache、shiftはcontext所有。構成も同じbundleへ載せ、App globalな現在構成で描かない |
| 変換書庫のDBキーが割れている | `src/app.rs:25559` (spreadはtree物理root)、`:25616` (pinは元root)、`:46678` (しおり元root)、`src/book_resume_db.rs:50` | 成立。ただしresume logical identityは既に元rootを使う。構成のために既存page編集キーを一斉変更しない |
| spread_separateへの影響 | `src/view_trim.rs:163`, `:226`, `:378`、`src/ui_view_trim.rs:831`, `:842`、`src/ui_fullscreen.rs:40760` | sideは画面Left/Right。ページordinalの奇偶ではない。構成slotを通してsideを確定する |
| Remote縦長解除は端末設定 | `crates/remote-web/web/app.js:13202` (`portraitSinglePage`)、`src/remote_ipc/container.rs:6075` (`force_single_page`) | 成立。旧app.js:11536を置換。本体のSplit、端末single強制を構成payloadへ混ぜない |
| session shiftは再入場で消える | `src/app.rs:25729`, `:44387`、`src/ui_fullscreen.rs:27987`、`src/app/gamepad_input.rs:6550`, `:6964` | 成立。§1.364は未実装。キーだけの抑止では不十分で共通action ownerで拒否する |

調査で重要な追加前提:

1. ファイル種類policyは本判定の**前**に作用する (`src/app/folder_scan.rs:286`)。
   構成の非表示、種類除外、一時facet絞り込みを同じ「存在しない」に潰すと、解除/直接移動が壊れる。
2. ZIP階層はwrapperをcollapseする (`src/zip_tree.rs:217`, `:368`, `:413`)。
   しおりのouter+entryから、実際にページを所有する実効階層を解決してから構成を引く。
3. 深いZIP階層のresumeは現在記録しない (`src/app.rs:56649`)。
   本機能に便乗してcross-book resumeを導入しない。階層内の保存時reanchorはidentityで行う。
4. Remoteはローカル閲覧と排他 (`docs/web-remote-plan.md:51`, `:57`, `:83`)。
   **取得をeditorのUIモーダルで止められるという初稿の前提を撤回**する。
   別スレッドの `SessionStateMachine::acquire` (`src/remote_ipc/session.rs:309`) が先にBeginAcquireを確定し、
   UIは後で全窓を閉じる (`src/remote_ipc/ui.rs:1081`)。この既存契約へeditorも含める (§8.1 / §10.2)。
5. 初回列生成以外にもitemsが変わる。実削除完了 (`src/app.rs:46052`) は
   `remove_items_batch` (`:42877`, `:42925`) で直接remove・世代更新し、自動reloadを抑止する (`:46055`)。
   identity表の既存remapだけでは新設explicit unitは直らない。投影ownerの行変更契約も必要 (§3.2)。
6. 集約からの直接閲覧はreleased経路 (`src/app/snapshot_ops.rs:1736`)。
   reader orderはsnapshot/collection/detailsを尊重する (`src/app.rs:68508`)。
   navは動画も含む (`src/ui_fullscreen.rs:13460`)。本を開き直すことを強制しない (§3.1)。
7. 退避列の復帰は直接代入 (`src/app/snapshot_ops.rs:1281`, `:1405`、
   `src/app/viewer_context_registry.rs:3403`)。現在のcurrent_folderだけの変更通知では不十分 (§10.1)。
8. メタ情報転送のcontainer familyはorigin一つ (`src/metadata_transfer.rs:2436`) で、
   source/cache混在はエラー (`:2163`)。importは共通target key (`:4396`, `:4695`)。
   構成をそのfamilyへ追加せず、専用の論理scope転送契約へ分離する (§7.3)。
9. 内容IDはFolderを持たず (`src/content_identity.rs:62`)、Folder itemは対象外 (`:169`)。
   画像本フォルダの外部移動を既存内容IDが復元するという初稿の前提を撤回する (§4.1)。
   保存成功時のEdit観測も必要 (`src/app.rs:25803`、§7.2)。
10. Remoteの方向/表紙操作は既存 `SetSpread` (`src/remote_ipc/ui.rs:2322`, `:2552`)。
    shift禁止だけからこの入口の廃止を推論しない。構成recordへの可視効果のある操作を提案する (§9.1)。
11. resumeのDBはpath一つにつき一行 (`src/book_resume_db.rs:128`)。
    writer受付 `persist_book_resume_with_identity` (`src/app/book_resume_meter.rs:181`) にcontext優先順位はない。
    窓ごとの再着地から書き込むとreload完了順が共有位置を決める。保存要求の一ownerだけが記録する (§6.1)。
12. 方向/表紙以外にも、PC HUDの末尾表紙・端配置・強制単独 (`src/ui_fullscreen.rs:25218`, `:25231`, `:25243`)、
    強制単独KeyAction (`:29564`, `:29577`)、Remote同等要求 (`src/remote_ipc/ui.rs:2327`, `:2331`, `:2350`) がある。
    現行setterはspread.dbへ保存 (`src/ui_fullscreen.rs:36922`, `:36973`, `:37022`、
    `src/remote_ipc/ui.rs:2603`, `:2644`, `:2703`)。Composedでは同じrecordの明示小編集へ接続する (§9.2、採択待ち)。

## 3. 所有境界と処理順 — items生成時の一回の投影

新規 `book_composition` にpure model/resolver、`book_composition_store` に永続owner、
`app/book_materialization` に本の列生成境界を置く案とする。
既存 `src/materializer.rs` は外部受け渡しの実体化であり、本の列生成とは責務が異なる。

```text
source enumeration / ScannedDir / ZipTree / PDF page manifest
  → 安全性検証・file_type_visibility・既存同名除去
  → BookScope + 元の名前/ページ番号順 + メタ情報
  → materialize_book(snapshot, composition)       ← 唯一の構成適用owner
  → PreparedBook { items, metas, identity_index, layout, complete_proof }
  → context付き既存install / items_generation
  → 一覧・ナビ・seek・spread/continuous・書き出し・F12・Remote
```

`BookMaterializationSnapshot` の入力は論理scope、物理ReadTarget/ZIP cache、採用済policy、
元ページmanifestとfingerprint、構成record/revision、一覧block配置。I/Oは準備worker、
pure投影は同worker、UIは完成結果を採用する。pending/resultは既存open requestと
`ViewerContextId`、要求世代、source/policy/revisionに相関させる。新たな待ちbool群を作らない。
構成なしは旧列生成をそのまま返す。キャッシュ済PDFの即時表示を全列挙完了待ちへ退行させない。
安全なraw manifestのmember名/階層metadataはpolicy投影と別にworkerが保持し、scope同定・Missing判定・
旧raw位置の照合に使う。除外画像を表示/decodeする許可や、重複除去済み行を再追加する入口にはしない。

投影は次を**同じ結果**として返す:

- 実在・採用済み・構成で可視な実ページだけを構成順で置いたitems/metas。
  ZIPの子container行は既存block順を保ち、構成は当該階層の直下ページblockだけを並べる。
  階層を跨ぐページ移動/子本の隠蔽/子ZipDirの並べ替えはしない。
- 元manifest identity→結果index、構成によるHidden、Missing、種類除外の解決表。
  `PreparedPageIdentityIndex` に適合するresume表も同時に生成する。
- indexへ解決済みのexplicit unitsとpresentation、構成revision、visible-page complete proof。
  hiddenも「登録済み」と数え、後から新ページとして再追加しない。
- 元manifestにあるが構成に未登録のページを元順で末尾へ追加する派生tail。
  推奨は各ページを中央Singleとする (§14-3)。寸法待ちや既存末尾との自動pairingを発生させない。
  読むだけでDBを更新しない。次の明示保存時にtailを構成へ取り込む。

構成のcomplete proofは「そのscopeの、policyで採用され構成で可視なページを重複なく全て投影した」
という証明。既存全items条件と混同しない。空の可視列も正当な結果で、全非表示を解除しない。
facet/検索欄でさらに減った列やRemoteの件数/bytes制限による途中列はcompleteではない。

必要なproducerのcutover一覧:

| 入口 | 変更方針 |
| --- | --- |
| 通常ZIP・直接RAR・変換ZIP初回/再読込 | `finalize_zip_enumerate` / `materialize_zip_grid` の直後ではなく、その列生成内部を共通ownerへ |
| ZIP enter/back、Ctrl+↑↓、DFS/slideshow | `zip_nav_show_current_level`へ集約。DFSの着地候補も構成後の可視ページで判断し、全非表示階層は読書着地を作らず次へ進む |
| 評価ZipDir、startup履歴prefix、Smart/Collection child | prefixを解決した後、同ownerからitemsを受け取る。後からraw列を代入しない |
| 画像本フォルダ | production folder listingから本projectionへ。本として適格かを同じownerで確定。本扱いOFF/混在はResultとして自動表示する推奨案 (§3.1 / §14) |
| PDFと共通Document経路 | page manifest/adoptionから同ownerへ。password/lease/cached adoptionの既存契約を保持 |
| しおり | entry検索はscopeとidentityのみ返す。item_index hintの生成をやめ、構成後のtarget resolverで解決 |
| Snapshot leaf / required open / delayed completion | 共通visibility target resolverを必ず通す。一致による直接openは維持し、Resultの順/自動組で開く。本列を新規生成する経路だけmaterialize_bookを通す |
| Remote ZIP列挙とpage context、PDF、folder listing | 同pure ownerをcore workerから使用。Web側に並べ替え・pairingの第二実装を置かない |
| 戻り用保存一覧 / F12 fork / parked context | immutable projectionと再生成recipe・scope/revision依存を保持。復帰adoption前に鮮度を照合 (§10.1) |
| 検索・★・snapshot等の集約leaf open | 結果一覧の検索順は維持。open前にそのページのscopeの構成visibilityを解決する |

親containerの代表サムネ・pin、類似索引の元内容、メタ情報エクスポートは「本の可視ページ一覧」と別。
構成で非表示にしても既存pinや画像編集記録を破棄しない。代表画像まで変える要求は本項にないため維持する。

### 3.1 本自身の列と結果列を混ぜない (採択待ち)

列の意味は生成ownerで `ProjectionRecipe::Book(scope, manifest)` または
`ProjectionRecipe::Result(surface, membership, order_policy)` に確定する。
各leafにComposed/Automaticを交互に適用する混合layoutは作らない。
Book資格は既存本分類を入口とし、さらに「一つのscopeの直下実ページblock + 既存子container block、
閲覧対象の動画/音声なし、結果surfaceではない」をownerで確定する。
PDFは番号manifest、画像folderは本扱いの実効判定、書庫は実効階層を用いる。
別scopeのleaf/動画/音声を持つ混在列はResult。子container行だけなら本のページblockと分離できるのでBookを維持する。
本扱いOFFは実効本分類がOFFになる場合の意味で、明示的な本置き場指定まで自動設定のOFFで解除する意味ではない。

| surface | 利用者に見える順・組・ページ送り |
| --- | --- |
| 本自身のZIP実効階層/PDF/適格画像本 | 可視実ページを構成順、explicit unitで表示。子container blockは従来どおり。詳細表示も構成順固定 |
| 上記の一時facet/一覧内絞り込み | 同じBook投影。既存unitの片側だけ残れば元slotのSingle。絞り込みを解除すると元unitへ戻る |
| ★固定/検索/評価/コレクション/サブ展開/スタック等の結果列 | membershipと既存結果順を保ち、従来の自動組で直接閲覧。別本の明示Pairの相方を探したり離れた位置から呼び寄せたりしない |
| 結果列の詳細sort | 従来の詳細sort順で自動組・送り。画像/動画/音声の従来の閲覧適格性を維持する |
| 本扱いをOFFにした画像フォルダ/本として不適格になった混在フォルダ | Result扱い、従来sort・自動組・動画/音声の既存閲覧を維持。保存構成は残り、本として再度適格なら再適用 |

Resultでも**構成の非表示判定は有効**。結果のmembershipは消さず、該当行に
「本の構成で非表示」と表示し、サムネイルの画像内容は表示しない。クリック/しおり/直接targetは
トーストで拒否し、送り/slideshow/自動pairingのnav eligible列からも除外する。
構成以外の実ページ/動画を落とさない。hidden判定は列生成workerのscope/revision snapshotから供給し、
UI描画中のDB lookupや現在mountのBookScopeから推測しない。これは本自身の「非表示ページは一覧に出ない」
と、検索等に記録が残っても開けないという要件を分けた案で、結果行の表示方法もG0で確認する。

構成のdirection/cover/明示pairはBookでのみ適用する。Resultには既存surfaceの自動設定が適用される。
結果順の説明は維持するが、**実際にBook投影へ移れるleafだけ**
「結果の順で表示しています。本の構成で読むには本を開いてください」と任意の「本を開く」入口を出す。
可否はworkerが対象scopeの実効Book資格とopen targetを解決して返す。
本扱いOFF/動画・音声の混在で元フォルダもResultになる場合はこの案内を出さず、
「この一覧では通常の並びで表示します。本の構成は保存されています」と示す。
本扱いを戻す/混在を解消して適格となった後に構成が再適用されることを説明し、開き直すだけで直ると案内しない。
結果をクリックすると必ず本へ入り直す案は、現行直接閲覧と送り順を変えるため採用しない。
その案へ変える場合は利用者承認が必要。Book適格性変更も未採択の推奨契約であり §14-12/13に分けて相談する。

sort lockは `recipe == Book && accepted composition exists` を既存
`page_order_locked_for_current_view` / `details_header_sort_active` の所有判定へ含める。
現行 `physical_page_order_locked` (`src/app.rs:12497`) やsynthetic判定 (`:46582`) だけに依存しない。
Bookには詳細sortを作用させず、Resultは構成の存在だけでsort lockしない。
分類・layout・sort lock・nav eligibility・shift eligibilityは一つのaccepted projectionから派生させる。

shiftはResult全体の組を動かすため、**nav eligible列に構成本のページが一枚でもあるResultは全列のshiftを禁止**する推奨案。
一枚でも含むかをprojection依存から得て、現在anchorだけで切り替えない。
構成本のページがないResultは従来の一時shiftを維持し、本別記憶を結果全体へ流用しない。
構成なし本のページも一緒にずらせなくなる不便があるため、§14-14で明示相談する。
承認なしに構成ありleafだけshiftを除外してunitを交互に再構築する機械を追加しない。

### 3.2 items行変更も投影ownerの更新である

PreparedBookの整数slotは `items_generation` と同時に採用される派生値であり、永続identityを代替しない。
初回installだけでなく、全行変更を同じ `update_projection` 境界に含める。

- **実削除**: `remove_items_batch` の既存old→new remap
  (`src/app.rs:42896`のshift、`:42905`のidentity表更新) をitems/metasと同じUI transactionで
  unit全occurrence・補助・anchor/selectionへ適用する。削除slotはNone、後方slotは同じ差分だけ移す。
  `[A,B],[C,D]`からBを削除した結果は `Single(A,元slot), Pair(C,D)`。AとCを組にしない。
  元manifest/identity解決表ではBをMissingへ更新し、構成DBのB参照は保持する。
  世代だけ進めて旧resolved_unitsを残すことは禁止。F5/workerを待たずunit/resume/exportは新投影を使う。
- **改名**: 成功経路 `src/ui_dialogs/rename_item.rs:172` は旧surfaceを解放し、migrationを起動し、
  親一致ならreloadを予約する (`:189`, `:202`)。専用PageRef移行をこの旧→新identity事象に含める。
  identity変更を反映できる列はpure remapを同時採用、再列挙が必要な列は既存reload完了まで旧投影を
  holdover専用とし、旧identityでのresume/export/編集を確定させない。単なるpage-edit rehydrate
  (`src/app.rs:45625`, `:45708`) だけでは構成identity/units更新の代わりにならない。
- **追加/外部変更/F5/policy・分類変更**: 新manifestとrecipeから共通ownerが再解決し、一括install。
  数値idxをidentityへ逆推測しない。Missingが同名identityで復帰したら指定unitへ戻る。
- **subset/詳細sort/復帰**: Bookは元投影のidentityに対するview投影、Resultは従来結果orderの投影。
  退避列を直接代入する経路も §10.1のadoption契約を通す。保存中にdraftの順をitemsへ差し込まない。

共通更新結果はitems/metas・identity/status表・layout・complete proof・依存revision・世代を一組として持つ。
公開consumerはその組のaccepted世代だけを見る。delete/rename/importは関連scopeの退避依存にも通知するが、
読み取りだけのfork/mount/closeは通知しない。resumeの互換rawは更新済元列、位置の正本はPageIdentity。

## 4. データモデル — 実ページと見せ方を分ける

### 4.1 BookScope / ページidentity

`BookScope { logical_root, level_prefix }` を使用する。rootは実フォルダ/PDF/元書庫、
ZIP prefixは閲覧と同じcollapse後の実効prefix。通常ZIP、nested ZIP、ZIP内directory、直接RAR、
cache閲覧で同じscope resolverを使う。階層間のfallback継承は**しない**。
root構成を子階層へ適用すると、本の別ページをhidden/missing扱いするためである。

永続キーは既存 `spread_db::container_key` と同じroot+実効prefix合成を
`path_key::normalize_keep_drive` で正規化する案。`StoreKeyNormalization::KeepDrive`を登録する。
DriveStrippedなspread/resumeと異なり、利用者が作った構成を別ドライブの同名本へ誤適用しないことを優先。
PDF/書庫ファイルのドライブレター変更・外部移動は既存content-identity restoreへ参加する。
**画像本フォルダはその対象外**。Folder内容hash/ledgerを新設せず、アプリ内改名/移動は既存key migration、
外部移動・ドライブ変更後は構成を自動適用せず、旧構成を残して明示メタ情報bundleのexport/importで引き継ぐ推奨案 (§14-10)。
移動予定なら移動前にbundleを書き出す。既に移動した場合は旧場所へ戻して書き出す方法を案内し、
名前が似たフォルダや一枚の画像の内容IDから親構成を推測コピーしない。
別案の「旧場所の構成を選んで引き継ぐ」UI/フォルダ内容IDは今回範囲を増やすので採択前に作らない。
root aliasは独自に増やさず、
初回/再入場/しおり/Remote/export全てがcollapse済prefixを得る共通resolverを使う。
policy変更でcollapse結果が変わる場合、保存scopeはraw treeの実効階層を基準に同定し、
投影treeの新collapse先へ別構成を誤継承しない (受入テスト必須)。

構成内部の `CompositionPageRef` は既存 `book_bookmarks::PageIdentity` と同じ三種類:

| 種類 | 保存内容 | 解決 |
| --- | --- | --- |
| 画像本 | container相対ファイル名/path | 検証済みcontainer内の元manifestへ照合 |
| 書庫 | outer rootからの完全entry_name | entry正規化後、scope直下memberか照合。nestedのchainも含む |
| PDF | 0-based物理page_num | 論理PDFの元ページ番号へ照合。items ordinalではない |

`book_resume_db::PageIdentity` は絶対logical identityで、しおり型と別物。
一方をもう一方のserde形式として読まない。共通純粋adapterで
`(BookScope, CompositionPageRef) ↔ resume identity ↔ GridItem/RemoteAddress`を相互変換する。
保存値にcache ZIP、texture ID、items index、ページの表示番号は入れない。
ページのcase/slash照合はしおりの規則を共有し、原entry表記はbytes読取に保持する。
同名directory/nested archive衝突の既存非対応 (2026-10-08決定) をここで拡張しない。

### 4.2 構成record

提案の論理形 (Rust実装指示ではない):

```text
BookComposition {
  schema_version, revision,
  spread_settings: { direction: Ltr|Rtl, cover: None|PageRef },
  units: [
    Pair { first: PageRef, second: PageRef },
    Single { page: PageRef, placement: Center|Leading|Trailing,
             companion: Vacant|Blank(BlankId)|FrontCoverSupplement(PageRef) },
    BlankBoundary { blanks: [BlankId] }
  ],
  hidden_pages: set<PageRef>
}
```

`first/second`は読み順。Leading/Trailingをdirectionで画面Left/Rightへ変換する。
したがってRTLを変えても実ページ順は反転せず、slot側だけ鏡映する。
`Center`にはcompanionを持たせない。Singleの片側とVacantは片側配置だけ、Blankは白い面を描く。
`BlankId`は構成内の編集用IDのみで、source addressやPageIdentityではない。

Pair/Singleを正本とし、「フラットpage/blank列 + pair上書き + singleton上書き」の三つを別々に保存しない。
flat列はunitを展開して得る派生値。実ページの通常occurrenceは構成全体でちょうど一つ。
hidden_pagesはその登録済PageRefのvisibility指定で、**unitsから参照を取り除かない**。
非表示/表示に戻すだけでは元Pair/slotを保持し、resolverがその側を欠落投影する。
これで非表示解除用の旧位置/旧Pair退避stateを別に増やさない。白紙と実ページを組んだunitはSingle、二実ページだけPair。
余分なblank単体/連続blankは `BlankBoundary` として編集では保持し、閲覧unitへは投影しない。
これにより「任意位置」の白紙を保存でき、白紙だけのページ送りや白紙だけのSingleを作らない。
末尾補助の表紙だけは `FrontCoverSupplement` の再利用を許す。普通のページ複製機能は範囲外。

保存検証では、種類一致、安全なrelative/entry、scope内member、通常occurrence重複なし、hiddenが登録済refの部分集合、
companion/placement整合、補助が指定coverを参照すること、ID一意、件数/文字列/JSON bytes上限を確認する。
cover指定があればその通常occurrenceはSingleとし、表紙を含むPair作成は先に表紙指定を外す同じdraft操作で行う。
coverなしの補助、cover自身への補助表示は拒否する。これで表紙設定とunitの可視効果を食い違わせない。
実体欠落は構文エラーではなくMissingとして保持する。利用者が作った構成を欠落だけで捨てない。

### 4.3 編集操作と初期化

作成時は元manifestの全ページと、その時点の実効mode/direction、保存回転後の横長判定、
§1.239端配置、§1.240強制単独、末尾表紙補助、session/saved shiftをworker snapshotとして取得する。
一覧facetで一部だけ表示されていても初期値は本全体から作る。
必要なheader寸法が未取得なら準備表示でworkerを待ち、暫定の縦長判定を確定値にしない。
既存自動builderとpresentation resolverを一回だけ使ってPair/Single/blank/補助に凍結する。
全体設定のFollowGlobalは保存せず、resolved値を保存する。
構成なしSingle/Splitから作るときのspread初期方向は既存本別/全体の値を解決してプレビューし、
Single/Splitの閲覧モード自体をペア指定と混同しない。

並べ替えは実ページ/白紙slotを編集draft内で移動し、**操作した連続範囲だけ**明示unitへ整形する。
Pairからページを抜いたら相方は元の側のSingle、貼り付け先の「この2枚を組にする」でPairへ変更する。
白紙挿入は選択slotの前/後にslotを作り、影響範囲の組替えをプレビューして適用する。
本全体の残りを暗黙にparity再計算しない。「自動の並びから作り直す」は明示操作として確認を挟む。
draftでの戻す/やり直しは在メモリの編集だけで、永続rollback logを作らない。

§1.239/§1.240/末尾補助のフラグは作成時に実際のunitへ変換され、その後の自動規則ではない。
補助表紙は特定unitに付くpresentation roleとして保存する。新ページ追加後もそのunitに残る案とし、
自動で新しい末尾へ移すかは§14で利用者判断を求める。
構成内の名称は「表紙の補助表示」、説明は「この単ページの相方に表紙を表示します」とする。
自動設定の「末尾表紙」から取り込まれたものでも、途中のunitへ残ったら末尾と呼ばない。

## 5. Spread / Single / Splitとキャッシュ

### 5.1 builderの入力と出力

呼出し側が複数設定を組み合わせず、入力を
`BookLayoutSource::Automatic(resolved_rules, optional_shift)` または
`BookLayoutSource::Composed(revision, resolved_units)` の一つにする。
Automaticは現行 `build_spread_display_units_with_page_alone` を維持。
Composedはexplicit unitsをindexへ解決し、隣接判定/landscape/shift/page_aloneを使って組み直さない。
明示Pairの横長画像もPairのままfitする。回転/補正/AI更新はgeometryだけへ作用する。

出力は現行のunit/occurrence/placement契約に適合させる:

- Pair → 二つのNavigation occurrence、読む先頭実ページのanchor。画面順はdirectionで解く。
- Single + Blank → `SpreadPair::Single`、実ページ一つ、`SingletonSpreadPlacement::LeftWhite/RightWhite`。
  白の寸法は既存§1.240と同じ可視実ページ寸法。blankのtextureやdecode要求はない。
- Single + Vacant → `SpreadPair::Single`、白い面なしの片側/中央配置。
- Single + CoverSupplement → 実anchorは一つ、補助は `SpreadPageRole::FrontCoverSupplement`。
  seek/履歴/ページ数/書き出しtargetは補助を含めない。
  補助表紙にも同じhidden/policy/Missing判定を行い、非表示・欠落のcoverを補助だけで表示しない。
  補助が不採用ならVacantとして実ページの側を保ち、DBの補助指定は保持する。
- Missing/種類除外/facetでPair片側が欠ける → 残る実ページを元slot側のSingleへ投影する。
  隣のunitと再pairしない。両側欠ければunitなし。MissingをBlankへ永続変換しない。
- BlankBoundary → 表示・ナビ・source demandなし。

単ページ閲覧はvisible実ページのflat列、blank/補助なし。分割閲覧はそのflat列を既存
`page_split::PresentationStep`へ渡す。Split halfはPageIdentityと別のpresentation identityを維持する。
Remote `portraitSinglePage/force_single_page`も同じsingle投影。端末の向きからDBを書き換えない。
連結読みは同じexplicit spread unitsを使い、anchor・seekとgeometryの対応を保持する。
現在のpage_edit pivotがSingleへ移っても構成recordは変更しない。

Book内の一時facet/検索欄は構成順で片側欠落を上記投影にする。
Resultは §3.1のAutomatic一つで、明示Pairを持ち込まない。動画の非pairable境界とnavigationを維持する。
LayoutSourceは一列に一つ、hidden/shiftのscope依存はrecipeから得る。複数本のComposedを継ぎ接ぎしない。

### 5.2 キャッシュのキーと失効

PreparedBookは `(scope, source_manifest_revision, policy, composition_revision)` を持つimmutable snapshot。
ここでcomposition_revisionはDB rowの数値だけでなく、serviceがscopeの採用値 (構成なしを含む) に付ける
`(service_epoch, publication_sequence)` を含むtoken。変更したscopeだけ新tokenを発行する。
解除→再作成で同じrow revisionへ戻っても一致しない。service再初期化ではepochを替え、
退避値を旧tokenで復帰させない。新たな永続tombstone/dirty storeは作らない。
構成変更は既存installで必ずitems generationを進め、index/memoを同時に作り直す。
行削除等のin-place更新でも §3.2のremap後に同じ世代を採用する。
Resultはmembership/order revisionと依存scope→構成revision (構成なしも含む) を保持し、
layout自体がAutomaticでもhidden eligibility/shift資格の変更でnav/unit tokenを失効させる。
`ViewerNavigationCaches` の世代/flowにはこのinstallが含まれるので、新たな「構成dirty」boolは不要。
unit tokenはLayoutSourceのtagとcomposition revisionを明示追加する。Composed branchにshiftは入らない。
nav列、items世代、Single/Spread/Splitの閲覧投影、direction、filtered projection identityを含める。
Automatic branchのlandscape epoch/page_aloneはそのまま必要。
geometry cacheはさらに既存寸法/回転/trim/scale revisionを使う。手動Pairのtopologyをlandscape epochで変えない。

旧holdover/passive snapshotはcapture時のunit・role・geometry・構成revisionを保持し、現在の設定から再描画しない。
source load/cancel/result採用は所有context/世代に限定する。blankがreadinessを永久待機させないこと、
単/二ページのrenditionをatomicに提示する現行規則 (`display-pipeline.md` §2.5.4) を受入条件にする。

## 6. 読書位置・しおり・履歴と非表示target

構成後のindexは永続identityへ直して記録する。現在のresume writerと
`PreparedPageIdentityIndex`を使い、構成保存後は同じidentityを新indexへ解決する。
構成本のrecordでは、互換用raw値を**構成前の正規列index**へ変換してidentityと同時保存する。
HUD/meterのordinalは構成後の可視列から別に算出する。rawに構成後indexを入れると、
旧版書込trigger・generic key移行でidentityが失効した場合に元列の別画像へ着地してしまう。
在メモリdedupの表示idxと、writerへ渡す互換raw値を混同しない。
単ページの相方や末尾補助からanchorを逆算せず、ナビanchorを保存する。
実ページ数/ordinalの分母は可視実ページ、blank/補助/子containerを含めない。
historyはこのordinalを表示し、再入場は通常container open + resume resolution。
`ReadingHistoryEntry.last_page` を新items indexとして使う新経路を作らない。
構成保存だけでhistoryを「読んだ」と更新しない。

旧resume raw-only記録の扱いは既存DBを壊さず次の順:

1. **初回作成前**の元の正規列でraw indexをidentityへ解決してsnapshotする。
   保存成功後の記録は §6.1の単一intentで行う。既存identityがあるならそれを優先。
2. 次回起動でidentity書込が未完了でも、構成がある本ではrawを**元列**に対してのみ解決し、
   そこから構成投影する。同じ番号の構成後別ページへ着地しない。
3. 種類除外/欠落で元列のrawを信頼できない場合は通知して復元拒否。
   構成なし旧記録の既存互換規則は変えない。released `book_resume.db` schemaの破棄/リセットはしない。

位置を投影結果indexへ解決する前に、この元列→identity adapterを通す。現行
`PreparedPageIdentityIndex::resolve`へrawを直接渡すと、policyが全可視の構成本で誤解決するためである。
構成解除時もidentityで解決し、当該raw値の意味を変更しない。

対象の解決結果は `Visible(index) / HiddenByComposition / ExcludedByPolicy / Missing / Invalid` とする。
これは通常のtarget解決に必要な区別であり、稀なDB failureの分類体系ではない。
`HiddenByComposition`ならエラートースト:
「このページは本の構成で非表示になっています。構成を編集すると表示できます」
を出してtargetを開かない。しおり、resume、★、編集/タグ記録は保持する。
Missing/種類除外は既存文言を維持し、「構成で非表示」と誤通知しない。

対象producerと消費先を対応させる:

- しおりpanel/browser/遅延完了、startup復元、book resume、閲覧/フォルダ履歴。
- 検索/評価/コレクションからのleaf、snapshot一致による直接open、required snapshot target。
- shell/二重起動/D&Dによる画像・書庫entry直接指定、F12の新規contextと再入場。
- Remote container resume、bookmark jump、直接page/thumbnail/AI/read/write addressの受付。

直接file openも親scopeの既存構成をworkerで照合してからpageを受理する。
「現在のitemsにない」だけの判定では、集約leafを一枚だけ開く経路やRemote direct addressを止められない。
既存open admission/target resolverに相関済み結果を渡す。
拒否時はopen/switchを確定しない。旧本・旧fullscreen/履歴・return routeは保持し、
先頭や隣ページへsilent fallbackしない。containerを一覧として開く要求自体は許可し、
非表示resumeのauto-fullscreenだけ拒否する。全非表示本は空一覧と「本の構成を編集」の導線を示す。

保存により現在anchorがHiddenになる**全更新対象context** (editor元窓、同一本の兄弟、
Result直接閲覧、退避contextの後の復帰を含む) は、同じ着地規則を使う案 (§14-5)。
各contextが変更直前に採用していた読書flat列を基準に、旧anchorより後の最初の新可視identity、
なければ前方を逆順に探した最初の新可視identityへ移る。同時に並べ替えても基準は変更前の順。
旧列の候補が全て消えたが新tail等があるなら新列先頭、可視navが全くなければfullscreenを閉じて空一覧。
Bookは本の旧構成順、Resultは旧結果/詳細sort順、動画/音声anchorはhidden変更の対象外。
新可視candidateはMissing/種類除外/現在facetも除く。これは**窓内のanchor変更だけ**で、
各窓のreload/復帰から共有resumeを書かない。historyを新たに読んだ扱いにもしない。
保存直前に元窓の移動先と「同じ本を開いている他の窓も移動することがあります」をプレビューする。
ユーザー操作による保存/解除/importの更新と、非表示targetを新規に開く要求の拒否を混同しない。

### 6.1 共有resumeは保存要求が一度だけ更新する

各窓のanchorはcontext所有だが、同じ本の永続resumeは一つ。保存/解除要求に
`ResumeWriteIntent::Keep | Reanchor { request_id, owner_context_id, resume_key, seed_identity, old_book_order }`
を一つだけ捕捉する。保存を押したcontextをownerとし、今mountされた窓、最後にreloadを終えた窓、
parked窓やsaved_itemsからownerを取り直さない。

- 元窓が対象本の実ページを読んでいるなら、そのidentityをseedとする。
  元窓が一覧だけ/別本なら、要求時点の既存resumeをidentity (旧rawは元列adapter) へ解決してseedとする。
  既存位置がない/安全に解決できない、深いZIP等の現行resume対象外ならKeep。保存だけで新しい読書位置を作らない。
- seedが新Book投影で可視なら保持。Hiddenなら保存要求に捕捉した**その本の旧順**で次→前→新列先頭へ解決する。
  現在元窓がResultでも、共有位置の候補を別本・動画・音声へ跨がせない。
  個別窓のfacet/結果順での着地とは分け、Book投影のpolicy採用済み可視実ページだけを使う。
  全非表示/seed欠落で安全な位置が決まらなければKeep。次回のhidden resume拒否は従来の本設計どおり。
- saveの準備workerが新投影とこの位置を純粋に解決し、composition commit成功replyの
  **要求完了owner一か所**が既存resume writerへ最大一回enqueueする。raw・identity・meterは同じ新Book snapshotから得る。
  enqueueは成功replyの処理内で各窓のreload/adoptionを開始する前に行い、後の通常操作のwriteより遅れて発行しない。
  commit失敗/cancelは書かない。各context reload完了を待ってownerを選ばない。
  同値なら既存dedupで省略し、独自の永続exactly-once台帳やretryを作らない。
- 既存open/navの `record_book_resume` (`src/app.rs:56647`, `:65174`、`src/ui_fullscreen.rs:21155`, `:38352`) は
  保存更新によるreload/adoption/reanchorを「新しい読書操作」として通さない。
  呼出し起点をtypedなProjectionRefreshとして既存install/open効果へ渡し、anchor/メモリのmeterを更新しても
  resume/history書込効果を発行しない。parked復帰・退避一覧の鮮度更新も同じ起点。
  per-contextのdedup値が違うことを、共有writerの優先順位代わりにしない。

意図した通常のページ送り等が再び許可された後は、従来の読書操作が位置を更新する。
更新中の操作はmodal/既存loadingで止め、旧reload返答を後の読書writeとして配送しない。
Remote取得がSavingを閉じた場合も保存済replyの同じintentだけを回収し、ローカル窓の終端から再記録しない。
§9の小編集は実ページ順/visibilityを変えないためKeepとし、reloadから記録しない。
metadata import等に伴う構成投影の背景更新も、新しい読書操作ではなくKeepを使い、表示窓/退避列の鮮度だけを更新する。
既存importの別sectionが位置を明示的に転送する場合はその既存ownerを維持し、表示reloadで位置を再上書きしない。
Q5の推奨は「次回の再開位置は保存要求の元位置に基づく一つ」であり、兄弟窓の移動先を奪い合わない。

## 7. 新しい本別ストアと既存ライフサイクル

### 7.1 保存

`<data_dir>/book_composition.db`、一つの `book_compositions(book_key PRIMARY KEY,
revision INTEGER, schema_version INTEGER, composition_json TEXT)` を提案する。
JSON内は相対PageRefのみ。全record一transactionのreplace/deleteで、units別テーブルを増やさない。
一覧は数万ページを想定し、worker一括照合 O(n)、索引map O(n)、JSON上限をschema/APIで共通検証する。
新ストアは未リリースなので**新ストアの旧schema migrationは不要**。
ただし既存released settings/resume/spreadやv7–v9 portable bundleまで移行不要と解釈しない。

App所有の単一service/workerがwriter。UIはPrepare/Save/Remove要求とreplyを扱い、同期SQLite/走査を追加しない。
読み取りsnapshotはimmutable Arc、contextごとに採用済revisionを保持する。
rename/purge/import/content restoreも同じmaintenance直列化境界を使い、第二writerで同時に書かない。
既存共通maintenanceがDBを直接更新する期間はserviceをquiesceし、完了後snapshotを更新する。
UIスレッドでjoin/lock待ちをせず、既存maintenance workerの完了を受ける。

保存失敗はtransactionで旧recordが残る。editorが存続していればdraftを表示したまま「本の構成を保存できませんでした」を通知し、
利用者の再操作かキャンセルへ戻す。自動retry、途中unit保存、独自quarantine/複数段回復を追加しない。
owner終了/Remote取得で既にeditorを閉じた場合は通知だけで、draftを復活させない (§10.2)。
破損/未知schemaならDBを作り直さず、通知・書込停止・構成本の閲覧拒否とする案 (§14)。
構成は利用者作成データなので、再生成可能なサムネcacheと同じ扱いにはできない。
稀なcommit後reload失敗は保存済recordを保持、通知して既存再読込または再起動を案内する。
表示を新構成で完成したと偽らず、失効した旧indexで編集/移動を続けない。
これ以上の回復が必要というレビュー指摘は、先に利用者へ割り切り案と費用を相談する。
明示「今回は構成を使わず開く」の回避案は §14-7の承認待ち。
選択したopen要求のcontext/scopeに限るAutomatic投影で、DB row/他contextのaccepted構成を変更しない。
shiftは許可せず、構成編集/小編集も書込停止中は受けない。次のopenへ選択を記憶せず、独自の復旧状態を永続化しない。

### 7.2 登録先チェックリスト

| 接続先 | 必要な処理 |
| --- | --- |
| `rename_key_migration::STORES` | KeepDrive/uniqueな本行を登録し、exact/`/`/`::`prefix rename/copy/probe/purgeの対象へ。DB未作成は正常不在 |
| 画像個別改名/移動 | しおりの`migrate_paths_at`同様、該当containerのunits/hidden/cover内のrelative PageRefをまとめてtransaction更新。page移動がscope外ならMissingを保持。root改名だけではJSON内変更不要 |
| archive内entry変更 | 本体から行う対応操作があれば同じPageRef移行を呼ぶ。外部rewriteはMissing保持で手動再対応。ページ番号から別entryを推測しない |
| 実ファイル削除purge | 本/rootの削除成功は構成行/prefixをpurge。ページ一枚削除では親構成をpurgeせず参照をMissingとして保持。非表示操作はFS/purgeを一切呼ばない |
| コンテナcopy / 内容ID復元 | generic `copy_stores_at`の本行とvirtual-prefix mappingへ参加 (`content_identity/restore.rs:414`)。relative payloadなのでcache mapping不要。destination既存構成を上書きしない既存conflict規則を使用。Folderには内容ID復元を約束しない (§4.1) |
| restorable probe / metadata cleanup | 構成も復元可能なuser dataとして数える。cleanupラベル、DB件数/clear、設定完全reset、data-dir移動・保存先説明へ登録。resetは既存明示確認の範囲で行う |
| 明示metadata export/import | §7.3の専用composition familyを追加。既存PortableContainerStateのorigin/挿入keyと分離。root/prefixはbundle相対、page refsは本相対。is_empty/has_view_state/summary/import changed flagsも更新 |
| metadata import refresh | owner/context/policyを捕捉し、関係する同一本scopeのprojectionを既存reloadで更新。別本のindex/cacheを捨てない |
| 自動sidecar | 今回は構成を追加しない。既存自動mimageviewer.datはページ編集等の用途。明示bundleが構成を運ぶ |
| F12 | registryのnew/default/destructure/swap/mount/fork/park/drop、親一覧/固定snapshotへprojectionを運ぶ。cache ownerはcontextごとに独立 |
| Remote | core engineにread snapshot/lookup登録、scope resolver・list/page context・visibility admission・payload登録。IPC版とWeb asset tokenを同時更新 |

構成保存/解除、rename/import/restoreが実際に変えたscopeだけに変更通知を出す。
読み取り専用open/closeはmutation-style invalidationを出さない。
content identityは元メディアの内容であり、構成revisionを内容hashへ混ぜない。
**構成保存成功時**には、要求時に捕捉したlogical rootへ `ContentIdentityTrigger::Edit` を送る。
現行 `record_current_container_content_identity` (`src/app.rs:25803`) の考え方を使うが、
reply時のmount/current_folderを参照しない。PDF/ZIP/対応元書庫は新規構成だけの保存でも
restorable ledgerを作る。nested保存もouter logical rootを観測する。Folderは非対応として送信対象外。
Remote小編集/importによる構成作成も同じ成功通知へ接続する。STORES/copy/probe登録のみで観測済扱いにしない。
純粋な本読みやmissing検出で永続構成を勝手に掃除しない。

metadata出力版は実装統合時の最新版+1 (調査時はv9→v10候補)。manifest/shard両方を更新し、
v7/v8/v9入力と旧字段変換を保持する。旧bundleの構成不在は「既存構成の解除」ではない。
importは既存preview/競合方針に従い、新payload全体を検証してから本行単位で書く。
imported JSONのabsolute path/`..`/unsafe entry/過大payloadは拒否し、既存データを変更しない。

### 7.3 構成専用のメタ情報転送family

既存 `container_key_base` はspread/trim等のために維持し、構成はそのorigin選択にも
`insert_container_state(container_target_key, ...)` にも混ぜない。
portable entryに別の `composition_family` を置き、論理entry rootに属する
`{ scope_prefix, composition }` の集合とする。root scopeは空prefix、nestedは検証済み実効prefix。
構成専用rootは常に元フォルダ/PDF/元書庫のbundle相対path。cache origin選択肢はない。
export照合はKeepDrive、importは搬入先logical rootをKeepDriveで正規化しscopeをjoinする。
通常のspread/trim familyは今のDriveStripped source/cache選択をそのまま使う。
scope prefix/relative refsを安全に検証し、受入先treeの実効scope resolverと整合させる。
cache rootを論理rootへ再分類して既存origin競合を避けるという改変はしない。

従って「元RAR側のroot/nested構成 + cache ZIP側のroot/nested spread/trim」は通常の正当なpayload。
構成以外の既存family内source/cache競合は従来どおり検出する。構成がその競合の理由になってはならない。
export選択でfolder entryが省かれる方式でも、構成対象のlogical folder entryをfamilyの搬送ownerとして
明示列挙し、子Image entryの構成として扱わない。画像folder bundleの受入試験でこれを証明する。

不在の意味はwireで三値を区別する:

- `composition_family`字段なし: 旧版または構成section未選択。既存構成を**保持**する。
- 新版でsectionを含み `{ scopes: [] }`: 選択logical familyは構成なし。
  import previewに「この本の構成を解除」と表示し、当該sectionの置換を利用者が承認した場合にだけ解除する (§14-15)。
- 非空scopes: 同じsectionの置換承認に従い、そのfamilyの選択scopeをreplace。
  export対象外prefixを巻き添えdeleteしない。payloadにscope coverage (rootのみ/明示prefix集合/family全体) を持たせ、
  そのcoverage内で「列挙されないscope=構成なし」と解釈する。

旧版の不在を新版の空へ変換しない。sectionを外してimportした場合も保持。
preview/summaryはset/解除の件数と対象本を示す。輸入元revisionをそのまま採用せず、
受入先の単一writerが内容の変更時にlocal revision/tokenを進める (§5.2)。
解除/再作成でも旧保持依存と同じtokenへ戻さない。
構成と既存spread/trimの各target keyを別々に計算し、
既存import batch/SAVEPOINT境界へ参加する。完了時は論理scopeの変更通知を発行する。

## 8. エディターUI — モーダルを採用する案

本全体のorder/indexを確定する短い操作であり、数十分のダウンロード管理と異なるため、
準備→編集→保存→reloadをモーダルにする。閲覧プレビューはdraftの専用領域で行い、
背景items/fullscreen stateは編集しない。常時モデルレスでは複数本/複数窓との割り込みが増えるため採用しない。

入口は本のHUD「…」と本一覧の右クリック「本の構成を編集」。空/全非表示本でも入口を残す。
状態は一つの `CompositionEditorState = Preparing | Editing(draft) | Saving | Reloading`。
owner context/scope/requestは状態内に持ち、show bool + pending bool + draft Optionを重ねない。
共通modal helperとローカルopen admissionへ同じ**mount非依存のeditor owner**を登録する (§8.1)。
既存UI modal判定 (`src/app.rs:21741`, `:21756`, `:37693`) だけをRemoteの確定境界とみなさない。

画面構成:

- 見出し: 本の名前、構成あり/新規、右開き/左開き・表紙あり/なし。
- 左: 元ページ一覧 (元名/物理番号、構成での非表示・Missingの表示)。元ページを見られるのはこの画面。
  file-type除外は採否policyを尊重し、必要なら名前だけ「ファイル種類の設定で非表示」と示す。
  この画面から種類設定を勝手に変えたり除外画像をdecodeしたりしない。
- 右: 編集中のunitカード。二枚/一枚/白紙/補助を見て組を変更できる。
  drag並べ替えとボタン「前へ」「後へ」「非表示」「表示に戻す」「白紙を挿入」「この2枚を組にする」
  「単ページにする」「中央/片側」、元に戻す/やり直すを用意する。
- 下の固定操作行: 「保存」「キャンセル」「構成を解除」。解除は本全体の非表示/orderを取り除くことを説明して確認する。
  「削除」は構成非表示には使わない。白紙の編集操作は「白紙を取り除く」。

virtualized一覧・可視thumbの予算付き取得で大冊も応答性を保つ。画像decode/DB/全件寸法読みをUIへ置かない。
背面wheel/key、slideshow、folder history、開く、F12、context activate、別editor、
ファイル操作/metadata import/settings変更をadmissionで止める。OSの別アプリ操作は制限しない。
OSで兄弟窓がfocusを得ても操作ownerを移さず、modal所在を案内する。
既存の兄弟描画用一時mountは許可し、その間もglobal owner参照は不変。Remote取得は拒否せず §10.2へ接続する。
外部からの新規openは旧要求を後で突然実行するキューに積まず、「本の構成を編集中です」で拒否する。

元manifestは外部FSから変わり得る。save前にworkerで再列挙/fingerprint照合し、変化があれば
保存を止めてdraftを保持、「ページが変更されています。読み直してください」と通知する。
編集継続中の自動merge/supersession/resumeは設計しない。利用者が読み直す操作で新draftを作る。
通常保存の最後の外部競合で参照欠落が生じても、identityを別画像へ代用せずMissingとして保持する。

Enter/Escapeは `dialog_enter_pressed` / `dialog_escape_pressed`、日本語single-line fieldを設ける場合は
`ime_focus` helper。IME確定でsave/closeを発火しない。新規アプリ操作のshortcutはKeyAction経由、
ドラッグやwidget内移動の固定操作は理由をkeymap文書へ記載する。
テーマは既存style helper、固定文字の未知glyph/絵文字は使わない。
1093×614 / 1366×728、UI倍率100/200%で操作行が画面内にあり、本文だけscrollするsnapshotを追加する。

### 8.1 開始の確定境界と終了のowner

App/registryの操作routerに一つのtyped local-operation参照
`CompositionEditorOwner { context_id, scope, request_id }` を置く。
draft/phaseはそのcontext bundle一つにだけ持つ。ownerはphaseのコピーやboolではなく所在の正本で、
どのApp投影をmountしても同じ参照からmodalを判定する。
context所有draftを現在mountのAppフィールドだけから探す実装は禁止。
`with_window_viewer_context`による兄弟描画 (`src/ui_fullscreen.rs:22457`) でも
owner Aを保持し、Bの入力を拒否する。Aを描画する時だけAのdraftへアクセスする。
main一覧だけにeditorを開いた場合もownerがあり、fullscreen_idx/窓数は所在の判定条件ではない。

ローカルeditor開始要求は、既存SessionHandleのlifecycle mutex確定境界
(`src/remote_ipc/session.rs:1547`) に短いLocal-phase admissionを追加する。
その同じlock内でLocalかを確認し、UI側のowner登録とPreparingの採用をO(1)で確定する。
session未起動時はUI routerの直列順だけで確定する。
lock内に走査/DB/worker待ち・viewport作成・context mount・再入するcallbackを置かず、登録後にprepareを送信する。
これは**BeginAcquireの拒否条件を増やす新admission gateではなく、既存取得境界でローカル開始を順序付ける拡張**。
sessionにeditorフラグや別のLocal/Remote ownerを作らない。startのphase確認とowner採用を
別々のsnapshot読み取りにするとその隙間で取得済editorを作れるので不可。
session mutexの保持範囲とlock順をG0で独立レビューする。

開始後にBeginAcquireが来たらRemoteが勝つ。prepare replyからEditingへ進む前・入力/save受付・
reload採用のいずれもrequest相関と既存session phaseを使い、取得開始後に新しいlocal操作を確定しない。
特に保存受付は開始と同じLocal-phase確定境界で、phase確認→Saving/ticket登録→非blocking channel送信を一度に行う。
worker transactionの実行はlock外。BeginAcquireが先なら未送信draftのままclose、save受付が先なら
全窓closeのbarrierが登録済ticketを必ず数える。phase確認後にlock外で初めてwriteを登録する隙間を作らない。
終了はownerを引いてcontext/viewportをretireする一つの `close_local_composition_editor(reason)`。
Preparingのworker replyをcancel/retire、Editingのdraftを破棄し、owner終了を兄弟へ移植しない。
Savingの送信済writeは §10.2のservice終端へ引き渡す。要求IDとscopeを持つ既存worker完了管理に回収責任を移し、
消えたeditorをreplyで復活させない。Saving後のキャンセルでDBをrollbackする仕様は追加しない。
通常cancel、owner窓close、全窓close、Remote取得、app終了をこの同じ終端契約へ接続する。

## 9. §1.364との境界

E側と共有する問い合わせは `resolve_book_layout_source(scope)` (Book列に対するもの)。
構成が存在すれば Composed、なければ Automatic + §1.364の解決済shift。
ResultはAutomaticで本別shiftを持ち込まない。shift可否はaccepted recipe/依存から一か所で派生させる (§3.1)。
構成snapshotの取得に失敗した状態を「構成なし」としてshiftを適用しない。

`queue_spread_shift_action`が見開きずらしを作る前に、この共通shift可否を見る。
Composedまたは構成本のページを含むResultならactionを消費、anchor/target/cache/DBを変更せず、
「この本の見開きは本の構成で指定されています。組を変えるには構成を編集してください」と通知する。
Resultには「この一覧には構成を指定した本のページが含まれるため、見開きをずらせません」と通知する。
四つのFsSpreadShift Action、カスタムキー、ゲームパッド/リング等で同じ結果にする。
Single/Splitの従来page移動はshiftではないので維持。
HUDの§1.364保存状態/解除は構成ありでは表示せず、「本の構成を編集」へ接続する。

**保存済shiftがある本へ構成を作る場合は利用者判断が必要**。
推奨案: 編集開始時の実効shiftを初期Pair/Singleへ取り込み、構成保存後はsaved shiftを休眠させる。
saved row自体を消さないので、保存跨ぎの二DB transaction/recoveryを必要としない。
構成解除時は §1.364の現時点の記憶ON/OFF・scope・identity解決規則で再評価する。
記憶ONかつidentityが解決できた場合だけ旧saved shiftを再び有効にする。記憶OFFなら休眠rowを
勝手にsessionへ持ち込まない。Missing等の旧anchorはEの採択規則に従い、同じraw番号へ代用しない。
解除確認ではその時点の実効結果を説明する。「必ず以前のずらしが戻る」とは表示しない。
作成キャンセルではshiftに一切触れない。記憶OFFのsession shiftも初期値に取り込むが永続shiftを新規作成しない。
「初期値に取り込んでsaved shiftを消し、解除後は基本見開き」の案も可能だが、別storeの消去失敗時の
扱いをE/Gで合意する必要がある。採択前にこの二DB処理を作らない (§14-1)。

構成のbinding設定の切替は2026-10-10追補で禁止されていない。
Single/Spread/Split切替は閲覧方法として維持し、右/左開きや表紙設定の永続ownerを二つ作らない。
構成ありのHUD方向/表紙切替は §9.1の同じ構成recordへの小編集を推奨する (§14-2/11、未承認)。
既存spread.dbだけを書いて無効な操作に見せない。

### 9.1 方向/表紙の小編集と可視効果 (利用者判断)

PC/Remote共通のpure操作をworker writerへ送り、一transactionで構成revisionを更新する。
構成editorはPC限定でも、この既存binding入口を残す。session排他によりPC draftとRemote小編集は並行しない。
PC editorが開いている時は背景HUD操作を受けず、editor内の方向/表紙widgetでdraftへ同じ操作を適用する。
この小編集の対象はBook recipeの当該構成。Resultのdirection/cover操作は従来の自動表示設定へ作用し、
そこに含まれる本の保存構成を変えない。入口ごとにcurrent anchorの親へ暗黙に書き込まない。

- **右/左開き**: directionだけを変更。reading order、Pair membership、SingleのLeading/Trailingを保ち、
  画面slotを鏡映する。補助表紙/白紙も鏡映する。表示が変わり、pairを組み直さない。
- **表紙なし→あり**: full Book投影の先頭の可視実ページをcover PageRefにする。
  Pair内ならそのPairだけを二つのSingleへ分ける。coverはTrailing+Vacant
  (LTRでは右、RTLでは左)、元相方は元slotのSingleにする。二つのSingleの元読み順は保つ。他のPairは一切ずらさない。
  coverが既にSingleならそのplacementをTrailingにする。明示Blank companionは残し、
  Centerからの変更等で不整合ならVacantにする。cover自身を補助として重ねない。
  非表示/Missingは選ばず、可視実ページゼロなら通知して変更しない。tailのページなら当該tailを明示unitへ取り込む。
  自動の末尾補助は追加しない。
- **表紙あり→なし**: cover指定を外し、そのページが今可視なら中央Single+Vacantにする。
  指定coverを参照する全FrontCoverSupplementはVacantへ変換し、白紙はBlankBoundaryへ残す。
  他の実ページ順・Pair/Single指定は保持し、前の相方と自動で組み直さない。
  hidden/Missingのcover指定も解除でき、存在しないページを出現させない。
- **別のページを表紙に指定**: 全editor内の明示操作。旧表紙解除→選択ページへ同じ単独化規則を適用。
  cover PageRefを変えるだけで古い補助参照を残さない。操作前後の影響unitをプレビューする。

同じ表紙あり/なしへの再送はno-opで、指定coverを新先頭へ移したり再単独化したりしない。
Remote `SetSpread` はmode enumのcover有無をそのまま全操作へ適用せず、
Single/Split要求は閲覧mode変更とdirectionだけ、Spread要求はdirectionと明示cover有無に分解する。
Single/Splitへ移るだけで保存coverを解除しない。Spreadへ戻る際はaccepted構成のcoverを反映してUIを表示する。
構成なしのSetSpreadは現行 `persist_remote_spread` / `core_reading_direction`
(`src/remote_ipc/ui.rs:2552`, `:4510`) の保存を維持する。

この表紙小編集は「表紙切替を維持するため、そのページの組だけを解く」例外を含む。
2026-10-10の「組はeditorで変更」との整合は**利用者承認が必要**であり、採択済としない。
復元用の旧Pair/placement保存を別状態として増やさず、表紙なしへ戻しても自動再pairしないことを説明する。
代案はPCで表紙操作をeditorへ接続し、Remoteでは構成あり本の表紙変更を制限する。
その代案は現在のRemote表紙切替を失うので承認なしに実装しない。
Single/Spread/Splitの選択は既存の閲覧mode保存ownerを使い、構成unitsを書き換えない。
modeに同梱されたdirection/cover操作はこの共通処理で分解し、同じpayloadの新旧設定を二DBで競合保存しない。

### 9.2 末尾表紙・端配置・強制単独の既存入口 (Q17、採択待ち)

PC HUD、`FsTogglePageAfterCoverAlone` / `FsToggleLastPageAlone` (`src/keymap.rs:1696`)、
Remoteの以下の要求を、**同じBookCompositionへのpure小編集command**へ集約する。
構成なし/Resultは従来の自動設定owner、Composedは構成worker writer。
初期化後のFollowGlobal/On/Offフラグを構成に併設せず、構成ありのUI状態は現在のexplicit unitから派生する。
全体設定変更では構成を再構成しない。

| 現行入口 | Composedでの明示操作 |
| --- | --- |
| PC末尾表紙 / Remote `SetFinalCoverSpreadPreference` | 下記の末尾unitへ表紙補助を付ける/外す |
| PC先頭/末尾単ページ配置 / Remote `SetSingletonSpreadEndpointPreference` | 指定端のSingleだけを片側/中央に変更 |
| 旧一括Remote `SetSingletonSpreadPlacementPreference` | 同じsnapshotで先頭/末尾のSingleへ適用。一つのrecord、一transaction。同じページなら一回だけ |
| PC表紙次/最終ページ単独、上記二つのKeyAction / Remote `SetPageAlonePreference` | 指定ページを片側Singleにする/中央Singleへ戻す |

対象は一覧facetやresponse budgetで変わる見かけの端ではなく、full Book投影の可視実ページから
要求受付時にPageRefへ解決する。先頭/末尾はそのflat列の端、表紙次は可視の指定coverの直後の実ページ。
表紙がなし/Hidden/Missing、次ページなし等では理由を通知して変更しない。
lastは表紙なしでも指定できる。初期取込後に「いつか表紙ありにしたら効く」未適用フラグを残さない。
tailが対象ならそのページを明示unitへ取り込む。requestはscope/PageRef/accepted revisionを捕捉し、
同じwriterが現行revisionとの一致を確認する。別窓の現在indexからtargetを取り直さない。
同じ操作を編集画面内で行う時はdraftへ適用、背景HUD/キーはmodalで拒否する。

**末尾表紙を付ける**: 可視coverがあり、末尾実ページと異なることを要件とする。
末尾がPairなら、そのPairだけを元順の二Singleへ分け、元相方は元slotを維持する。
末尾ページは既存の片側配置を維持、中央ならLeadingへ置き、反対側にFrontCoverSupplementを付ける。
白紙companionがあれば同じ位置のBlankBoundaryに残す (表紙表示への置換で白紙指定を消さない)。
他のPair/途中の補助表紙は変更しない。**外す**操作は現在末尾unitの補助だけをVacantにし、
実ページの側を保つ。分けたPairを自動復旧しない。途中に残った補助はeditorの「表紙の補助表示」で編集する。
付ける操作でcover不在/同一ページなら通知して無変更。外す操作はcoverがHidden/Missingでも保存補助を外せる。
補助ありへの同値再送はno-op。

**端配置**: 現行端side resolverのLTR/RTL・cover条件を一回だけ解決し、Leading/Trailingへ保存する。
ペア全体に自動規則をかけない。対象の保存unitがPairなら配置対象外と明示し、unitを分割しない
(従来も端配置だけではPairに作用しない)。Singleの片側変更はcompanionを反対側へ移す。
相方のMissing/非表示による一時Single投影でも保存Pairは維持し、手動の組を置換しない。
中央指定はCenter+Vacantにし、白紙は同じ位置のBlankBoundaryへ残し、補助は外す。
補助/白紙が外れることをUI説明に含める。旧一括要求で一ページだけなら先頭指定を採用し、末尾指定を重ねない。

**単独にする**: 対象がPairならそのPairだけを元順の二Singleへ分ける。
相方は元slotのVacant。対象の元slotを保ち、白紙companionを一つ付ける。
中央Singleなら表紙次はTrailing、最終ページはLeadingへ置き、白紙companionを一つ付ける。
既存片側Singleはその側を保ち、Vacantなら白紙を付け、既存Blank/表紙補助は維持する。
**単独指定を外す**: 既存Pair/中央Singleならno-op。片側Singleを中央Single+Vacantへ変更し、相方の白紙/補助を取り除く。
この操作は白紙相方を明示的に取り除く指示であり、旧Pair/自動Pairには戻さない。
指定cover自身の単独配置は表紙設定が所有するため、最終ページ=coverの場合は対象外と通知する。

KeyActionのtoggleは「片側Singleなら中央へ、Pair/中央Singleなら単独にする」と現在unitから決める。
HUD/RemoteのOn/Offも同じ明示操作で、隠れた強制単独bool/過去の組の復旧logを持たない。
既存の手動Singleにも同じ意味で作用するため、構成ありの文言/tooltipで「解除しても組へは戻らない」と説明する。
片側配置/補助が残る単独表示を「無効」と見せず、状態と実際のunitを一致させる。
FollowGlobalを受けた場合は要求時の全体値を一回解決して上記commandへ変換し、
UIは保存後の明示結果を表示する (FollowGlobalを選択状態として残さない)。
Single/Split閲覧中でもrecordは編集でき、見開き投影へ戻ると反映する。

全操作は §9.1と同じtransaction・Edit観測・revision通知・context更新・Remote cache失効を使う。
同値/対象外ではDB/revisionを変えず、対象外はPCの説明/トーストとRemoteの理由付き応答で知らせる。
新操作では実ページ順/非表示/白紙source/readinessを変更せず、resume intentはKeep (§6.1)。
全体へのautomatic builder再実行、既存spread.dbだけへの無効な書込、同じ設定の二重永続ownerは禁止。
Pairの局所単独化、Offで組へ自動復帰しない意味、FollowGlobalを継続しない意味はQ2/Q11に加える例外なので
Q17で利用者承認を求める。代案のeditor誘導/Remote制限は既存操作の手間・機能を変えるため、承認なしに採用しない。

## 10. F12 / 多窓 / Remote

### 10.1 ローカルcontext

DBサービスはApp共通、`PreparedBook/layout/identity map/revision/unit cache/editor draft`はcontext所有。
F12 forkはimmutable構成snapshotを共有できるが、nav/cache/queue/cancel/generationは複製したcontextのowner。
fork/park/mount/deposit/close自体はDB writeや sibling reloadを発生させない。
保存replyはrequestを発行したcontextへ配送し、今mountされているcontextを要求元と推測しない。

save後は**現在列だけでなく全保持projectionの依存scope**を照合する。
保存対象と同じscopeを持つBook contextは既存本reload、Result contextはmembership/結果順を保って
visibility/nav eligibilityだけを再解決する。anchor PageIdentity、閲覧mode、filter、return route、zoom/scaleを捕捉する。
parked contextも既存metadata import refresh同様のcontext identityで扱う。
現在anchorがHiddenになれば全contextで §6の着地規則を使う。
別本/別階層は不変。同じscopeの旧snapshotは新構成を完成表示と扱わず、既存holdoverとして使う。
窓の無条件閉鎖・zoom全resetで解決するとreleased操作を削るので採用しない。
reloadが既存routeでは状態を保存できないと判明したら、実装を拡げる前に設計担当/利用者へ相談する。

#### 退避projectionの鮮度と復帰adoption

snapshot.saved_items、固定list_items、pre-search/return grid、F12 direct_ownerの元一覧、
parked bundle、folder-historyの保持列を同じ契約で登録する。保持値は単なるitemsコピーでなく
`{ recipe, accepted_projection, dependencies: scope→(composition_revision, source_manifest_revision, policy_revision) }`。
構成なしも依存として記録し、
後から作成された場合を検出する。保持projectionへの変更通知はrevision差で表し、別のdirty boolは持たない。
source_manifest_revisionは当該scopeの受入済manifest更新/実削除/改名のpublicationを表し、
contextごとのitems_generationをscope間比較に使わない。新しいFS watcherは追加せず、既存変更検知/F5の
列生成結果をownerで発行する。戻り時の既存external-change確認もこのownerへ接続する。

復帰は必ず `adopt_retained_projection` を通す:

1. 現在store/source/policy snapshotのrevisionと依存を比較。一致なら既存immutable値を一度だけinstall。
2. 不一致ならBookは保存recipeのscopeから既存reloadを要求し、Resultは**固定したmembershipと
   order policyのまま**hidden eligibilityを再解決する。元book itemsへの再適用による二重並べ替えをしない。
3. 新accepted projection採用後にidentityでselection/anchorを復元。Hiddenなら §6へ。
   scroll/index cachesは新世代、thumbはidentity一致分のみ再利用する。

`snapshot_ops.rs:1281` の固定解除、`:1405`の固定一覧へ戻る、
`viewer_context_registry.rs:3403`のF12元一覧復元を含め、items直接代入からこのadoptionへcutoverする。
鮮度確認にworkerが要る間は既存loading表示で、旧列をholdoverとして描けても操作を確定しない。
固定membership、snapshot origin/source label、pre-search origin、return routeをreloadで破棄しない。
変更された本から離れているcontextを即座に本へ移動させず、保持値は次の復帰時に更新する。
保存中はローカルmodalで新しい復帰要求を止め、保存後の鮮度チェックへ揃える。

editor terminal closeは §8.1/§10.2に従う。indexを使うdraftを別窓に移植しない。
detached述語/viewportに触る前にはClaudeCodeと独立Codexレビューで
「context投影の所有を拡張する構造的修正であり症状パッチでない」ことに合意し、
実装範囲と理由を `detached-rework-plan.md` §11へ記録する。
新規detached bool、geometry捕捉、repaint/retry/time窓による修復は不要。

### 10.2 Remote

coreのContainerEngine workerが同PreparedBookを生成する。Webは返されたentries/PageGroupを表示し、
構成のorder/pairを自前で再現しない。実addressは元root+entry/物理PDF番号のまま。
Bookには `has_composition`、revision/accepted layout identity、構成の実効bindingを追加する。
Resultには列recipeのtag、leafごとのhidden/構成依存、全列のshift availabilityを返し、
一つのhas_compositionだけで混在を表さない。両者ともaccepted projectionから生成し、
既存presentation/anchor/singleton placementを共有する。白紙のaddressを発明しない。
件数/response budgetで切詰める前にfull projectionを作り、切詰め後はcomplete=falseとする。
Pairの片側が応答外なら残る実ページのslotを保ち、見かけの末尾へ表紙を付け足さない。

構成editorの全体編集は今回はPCのみを推奨。Remoteは構成で読む、非表示を拒否する、
§9.1の方向/表紙小編集で既存SetSpread、§9.2の小編集で周辺三系統の要求を維持する推奨案
(§14-11/17、未承認)。
Remoteの全体編集UI/APIの追加は別段階/要求とし、本機能のPC編集と同時に作らない。
書込は必ずIPC→core所有service。read-only connection/workerから書かない。

#### Remote取得は全ローカル窓closeへeditorを含める (推奨、§14-9)

現在の「後から操作した側が勝つ」を保ち、editor中でもBeginAcquireを拒否しない。
`poll_remote_session`のAcquiringRemote分岐 (`src/remote_ipc/ui.rs:1081`) の全窓closeに
editorのterminal closeを含める。`close_all_detached_viewers_for_mode_change`の
`had_detached == false`早期return (`src/app.rs:58163`) やfullscreen_idxなしでeditorを見落とさない。
全ローカルsurfaceのclose coordinatorがglobal editor ownerを無条件で列挙し、
context close関数は同じterminal処理を使う。別modal窓だけ残す設計にしない。
`local_viewer_close_state` (`src/app.rs:73834`, `:73863`) はeditor owner/viewportと
送信済構成writeの終端も含めてClosedを判定する。mount中のApp fieldsや窓数を代用しない。

取得順の証明 (startは §8.1の同じsession mutexで確定):

| 確定順 | 結果 |
| --- | --- |
| editor開始→BeginAcquire | ownerは既にregistryに存在。Remote phaseがlocal操作を止め、UIの全窓closeがeditorもretire。未保存draftを破棄しmainへ通知 |
| BeginAcquire→editor開始要求 | sessionはAcquiringRemoteなので登録を拒否。prepare/viewportを作らず「Remoteから操作中です」。取得後にprepare replyで開く余地なし |
| editor準備replyと取得が競合 | startの順序は変えず、取得後replyはretired requestとして回収。Editingへ復活させない |
| 兄弟Bを一時mount中に取得/開始/close | global ownerはAのまま。registryのcontext IDでAを閉じ、BのApp投影にdraftがないことを「editorなし」と解釈しない |

Preparing/Editingは取消して「Remoteからの操作に切り替えたため、本の構成の編集を閉じました。未保存の変更は保存されません」。
Savingは送信済transactionを取り消さず、**窓/draftを先に閉じ、serviceの一回の完了を収穫してからFinishAcquire**。
commit済なら構成は保存されている。成功は「本の構成を保存し、Remoteからの操作に切り替えました」、
失敗は旧record保持と保存失敗の通知。消えたdraftを復旧・自動retryしない (§14-7/9)。
Reloadingは保存済recordを維持し、local reload/返答をretire、Remoteが新snapshotを列挙する。
service drainは既存AcquiringRemoteのclose barrierへ接続し、Remoteの最初のenumerationは
保存writeが終端になった後。既存read workerは `wait_until_active`
(`src/remote_ipc/pipe.rs:1797`) を通り、AcquiringRemoteはそこで待つ
(`src/remote_ipc/session.rs:2501`, `:2524`)。write workerも同じ待ちを持つ (`src/remote_ipc/pipe.rs:1338`)。
この既存activation境界を使用し、取得応答がActiveというだけで列挙を先行させない。
現在の30秒取得失敗処理 (`src/remote_ipc/ui.rs:1130`付近) は維持し、
追加の待ち時間/retry/保存完了後のlocal窓復元を作らない。
owner窓終了/Local返却/取得abortでも旧requestを新ownerへ配送せず、replyはserviceで完了させる。
app終了途中のcommitはDB一transactionの従来の保証に任せ、永続draft回復は要求しない。
SessionHandle mutexを取った短いUI確定処理からservice状態のblocking lockを取らず、
worker側からregistry/sessionへ逆向きlockを取らない。通知/完了はchannelでUI routerへ返す。

代案「編集中は取得拒否」はreleased契約を変え、editor start/BeginAcquire両側のownerを
session stateへ追加する設計が要る。推奨案へ混ぜず、利用者が §14-9で選んだ時だけ別の設計レビューを行う。

PC変更の端末反映は既存session acquire時のcache破棄で扱う (`web-remote-plan.md` §2.2/§12.16)。
全editorをPC限定にし、Remote小編集をcore serviceが通知する契約なら、新data_version watcherは要らない。
Remoteセッション内のbinding変更は既存remote generation/invalidation通知へ登録し、
container/group cacheも同じaccepted layout revisionで失効させる。
force_single/端末orientationはこのrevisionを増やさず端末ごとのprojection keyへ載せる。
IPCは調査時v69から統合時の最新+1、Web asset tokenとstrict handshake/testを一緒に更新する。

## 11. トリム・変換書庫・出力の契約

### 11.1 view-trim spread_separate

本別 `spread_left/right` は画面の側への設定として保持し、ページidentityへ焼き込まない。
pairを変えてページが反対側へ移れば、その新しい側のmarginが掛かる。RTL変更も同じ。
page overrideは従来どおりページ固有で、本設定との優先順位は `view_trim::stored_view_trim_bbox`へ集約する。
構成保存でmarginをswap/average/resetしない。crop/mask/annotation/edit空間も変更しない。

Single+blank/片側配置も**spread閲覧中のslot側**で本別spread marginを選ぶ案。
普通のSingle閲覧/編集pivotはsingle margin。blankにtrim/auto-trim/partner decodeを要求しない。
Autoの二枚上下harmonizeは実partnerがあるPair/補助だけ。
現行Single rendererはsingle bboxを使う経路があるので、単なるSpreadPair::Singleへの変換だけでは不十分。
slot/presentation modeを共通trim resolverへ渡し、local/Remote/continuous/holdoverで揃える。
§1.240既存singletonにまでこのtrim変更を拡げる場合は既存仕様との相違を別途相談し、
構成branchの追加に便乗してreleased本のtrimを変えない。

### 11.2 変換書庫

`logical_root` (元RAR/7z/LZH等) と `physical_read_target` (cache ZIP/直接RAR) をsnapshotで分離する。
current_folder + archive_source_overrideが同じ受入済cacheを指す場合だけ対応を使う
(`src/app/snapshot_ops.rs:2241`)。App globalなcache aliasを別contextへ当てない。

- 構成/構成PageRef/しおり/新target lookupは元root+entry。
- デコード、既存回転/補正/trim等page DBは現行物理cache側のキーを維持。
- 自動spread値を作成初期値へ取り込む時だけ現行cache側 `spread_container_key` を解決する。
- 再変換後cache path変更でも、entry identityが同じなら同じ構成へ解決する。
  entry名/階層が変わればMissing/新tailであり、items番号から推測しない。
- 元書庫が消えても受入済cacheのF5閲覧は現行どおり維持し、既存overrideから構成を引く。
  source currentnessを新たに毎open必須にしてcache-only閲覧を退行させない。

### 11.3 書き出し・製本・metadata bundle

通常一覧の一括書き出し/製本追加は `grid_selection_indices` が返す構成後index昇順。
構成非表示はitemsにないので選ばれず、白紙/補助もsourceに変換しない。
Resultの非表示markerはitemsには残るため、selection→実Source確定にも共通visibility resolverを通す。
markerは書き出し/製本追加の選択対象にしない。Resultの可視選択は従来の出力順・直接閲覧を維持し、
別本の構成順へ並べ替えない。本全冊追加は必ずBook projectionを生成して渡す。
workerへは既存の実ページSource/編集snapshotを渡す。元書庫を書き直す機能は作らない。
全冊追加の入口がraw列挙している場合も「表示中本の可視実ページを渡す」共通snapshotを使用し、
raw constructor経由でhiddenを取り込まないことをテストする。

製本された出力は可視実ページを連番へ焼き込む。構成DB/白紙/explicit pairを製本先へコピーしない。
これが「製本は構成作成対象外」と両立する。export順は構成順を採用 (元順も許容という要件の上位側)。
一枚書き出し/見開き合成/外部ツールは既存Navigation roleだけを使い、white canvasを画像に追加しない。
画面captureは見えている白い面を含む既存画面領域の意味を維持する。
metadata bundleは可視一覧選択からの画像exportとは別で、**非表示参照・Missing参照・白紙を含む構成全体**を運ぶ。
二つの「bundle」(メタ情報転送と製本用画像集合) を同じ処理にしない。

## 12. 消した状態の組み合わせと採否理由

| 消した組み合わせ | 方策と理由 |
| --- | --- |
| 編集中に別本open / 戻る・進む / F12 / sibling activate | mount非依存のlocal modal owner。元に戻す/supersede draft/resume機構を作らない |
| editorとRemoteの二重操作 / 取得拒否をする別gate | 同じsession確定境界で開始を順序付け、後のRemote取得が既存全窓closeを使う。draft永続退避/自動再開なし |
| itemsと別の読み順だけ構成順 | 本列生成ownerで一回投影。通常ナビ・単ページ・seek・連結を同じitemsへ |
| PC列挙とRemote列挙の異なるorder/hidden/pair | same PreparedBook。Web独自pairingを増やさない |
| 構成 + 一時shift + saved shiftの合成 | typed LayoutSource二択。Composedでshiftを解決/書込しない。解除時はEの現在の記憶規則へ戻す |
| 異なる本の明示Pair / Result自動pair / 動画の混合layout | 本自身だけComposed、結果全列はAutomatic。hidden/shift資格だけをscope依存から派生 |
| 任意page/blank列 + explicit pair overrides + singleton flags | explicit units一つ。flat列は派生、blankはsourceではない |
| 自動landscape/page_alone設定変更 + 保存済手動Pair | 初期値で自動規則を凍結。以後はgeometryだけ更新 |
| 閲覧mode/orientation/Splitが本の永続ペアを変更 | view projectionを分離。single/splitは同じ可視flat列 |
| 保存中のitems in-place並べ替え + index編集cache | draft→commit→既存reload。削除等は共通remapでitems/units/identityを同時更新 |
| 新構成で表示中 / ★固定・F12退避に旧構成 | 保持recipeとrevision照合、一つの復帰adoption。固定membership/return routeは維持 |
| 個別本reloadとF12独自rebuildを別々に実装 | 既存context付きreloadへ接続。scopeに関係するcontextだけ更新 |
| 読取open/closeでmutation invalidation | readはrevision採用だけ。実際のwrite/rename/importだけ通知 |
| 書庫cache keyと本のidentityの混在 | logical/physical typed snapshot。ページ編集DBは既存keyのまま |
| 元書庫構成 + cache spread/trimの転送origin競合 | composition専用logical family。既存container origin判定へ参加させない |
| Folder内容ID / 一枚の画像ID / 類似pathによる親構成推測 | 外部folder移動は明示bundle転送。自動識別・復元元選択UIを今回追加しない |
| cover切替の元Pair復旧log / metadataだけ変わるcover | 影響unitだけ単独化し、戻しても自動pair復旧しない小編集案。可視効果を明確にして利用者相談 |
| 兄弟窓reloadの完了順 / 共有resumeの優先順位 | 窓内anchorと保存要求のresume intentを分離。保存完了ownerから最大一回、退避/parked更新は書込なし |
| 末尾補助/端配置/強制単独の自動フラグ + 明示unit | 既存入口をunitへの小編集commandにする。状態はunitから派生し、全体規則・旧Pair復旧logを併設しない |
| 新storeの移行/旧設計試作データ互換 | 未リリースstoreは新schema一つ。released既存DB/bundle互換は維持 |
| 稀なDB failureのretry/途中保存/rollback後のrollback | record一transaction + draft維持/通知。独自回復網は利用者相談前に作らない |

採用しなかった簡略化: 構成保存だけを理由に全窓を閉じる、全設定reset、PDF即時表示を全件待ちへ変える、
構成なし本のshiftを取り除く、読み取り時の新tail自動保存。いずれもreleased機能の退行、
利用者データ変更、または別contextへの不要な副作用がある。
Remote取得の全窓closeはそのreleased契約を維持する場面であり、保存更新の全窓closeとは別。
Result中のshift制限、表紙小編集、本扱いOFF/混在後の表示は不便/仕様差を伴うので、承認待ちとして残す。

## 13. 段階・担当範囲・受入テスト

設計担当が下記chunkを確定し、各chunkのwriterを一人にする。実装と独立レビューは別context。
本書では追加subagent/レビュー実行をしていない。設計handoff後にcoordinatorが独立レビューを手配する。
ライン E の§1.342統合前にはproduct実装を始めない。§1.364と契約が未決のままshift storeを設計しない。

### G0: 設計採択・E統合後の再照合

成果: §14の全回答、独立再レビュー、scope resolver/DTO/shift責任分担、detached構造合意。
受入: 本書§2のsymbol/lineを新masterで更新、下記E接点の差分を読み、仕様の矛盾を潰す。
editor owner登録・保存ticketとsession mutexのlock順/短い保持、全close barrier、保持recipeの復帰adoptionを
構造レビューする。初回REVISEをこの設計修正だけでACCEPT扱いにしない。
小編集の表紙unit変更、Resultのshift、OFF/混在、Folder移動、転送の明示解除を未回答のまま実装briefへ確定しない。
共有resume intentのowner/seedと背景更新Keep、§9.2の周辺小編集/Q17も採択対象。
依存順は **G1 (構成なしの境界) → G2 (pure model/fixture) → G3 (構成ありの適用/行変更)**。
G1完了の条件へG2以降で導入する型/構成ありfixtureを先取りしない。

### G1: items列生成の統一 (構成なしでの互換を先に証明)

所有: `app/book_materialization`、ZIP/folder/PDF/しおり/snapshot/Remote producer。
受入fixture:

- 数字名/日付/評価sort、通常folder/画像本/製本、PDF、ZIP wrapper・nested・直接RAR・変換cacheで旧順と一致。
- ZIP root/enter/back/DFS/評価ZipDir/bookmark/snapshot/Remote listとpage contextが同じscope/orderを返す。
- policy before book classification、same-name除去、cached PDF早期adoption、password cancel、古いworker不採用。
- context Aのread-only open/close/forkがBのitems/generation/queue/cancel/cacheへ変更を出さない。
- visible_indices昇順、itemsとmetas長/対応、complete proofとRemote切詰めの境界。
- 行変更入口を一覧化し、構成なしで実削除完了→remove_items_batchの既存items/identity remapと
  自動navの世代更新が従来と一致することを証明。新explicit unitsのremap fixtureはG2/G3へ置く。
- 構成なしで結果への直接snapshot open、動画/音声を含む結果順・nav、詳細sort、本分類の既存挙動を維持。
  構成ありのsort lock/混在/shift fixtureはG3で検証する。

### G2: model / store / lifecycle

所有: pure model/resolver、store service、rename/purge/content restore、metadata transfer。
受入:

- round-trip、Pair/Single/blank/補助/hidden、重複・unsafe・unknown schema・上限拒否、一transaction rollback。
- pure model導入後に `[A,B],[C,D]` のB削除remap・Missing保持、周辺小編集commandのfixtureを作る。
  G3/G4はこのfixtureを使用し、実producer/installと完了順を検査する。
- root/prefix rename、画像一枚改名、scope外移動/ページ欠落、book削除とpage削除の違い、KeepDrive衝突なし。
- 内容IDcopy後のrelative参照、destination構成を保持、cache再変換でもlogical key一定。
- 新formatとreleased v7/v8/v9入力、nested状態、旧bundleの構成不在で既存構成を消さない。
- readでDB writeなし、clear/reset/cleanup登録漏れなし、変更通知の対象scope限定。
- 構成だけ新規保存、fullscreen閲覧/読書履歴なし→PDF/ZIP/元変換書庫を外部移動→内容ID復元候補。
  成功reply時に兄弟mountでも保存したlogical rootのEdit ledgerがある。Folderは自動候補にならない。
- source KeepDrive構成 + cache DriveStripped spread/左右trimの同居を、root/nested双方でexport/import往復。
  既存origin競合を誤発火せず、構成は搬入logical root、spread/trimは従来cache key。
  画像folder entryの搬送、旧字段なし/section未選択/新版明示空/部分coverageの保持・解除をpreview込みで検証。

### G3: 構成適用・spread/read position・shift境界

所有: common projection、`ui_fullscreen.rs` builder/cache、resume/target resolution、Eとのshift API。
受入oracle:

- 初期値が現行自動構成と一致: LTR/RTL×cover有無×1/2/3/4/5/多数ページ×横長/非pairable境界×
  §1.239両端×§1.240両切替×末尾補助×shift。既存テストの期待値を弱めない。
- `[A,B],[C,D]`→B非表示で `A元slot; [C,D]`、B再表示、順交換、白紙挿入、blank-only境界、全非表示。
- explicit Pairは回転/寸法後着で組が変わらず、Single/Spread/縦横連結/Split/portraitSingleで同じ可視実列。
- 新tailがhidden登録を復活させず、Missing復帰を元identityへ戻す。再起動/reloadも同じ結果。
- resume identity/旧raw-only/書込中終了相当、history ordinal、bookmark hintが別ページへ着地しない。
  composed recordのrawが元列indexであること、同値raw-only更新trigger後とgeneric key移行後も誤着地しない。
- hiddenへの全入口で通知・open拒否・旧context保持。Remote direct page/AI addressでも拒否。
- 全FsSpreadShift Action/カスタム割当/ゲームパッドのno-op+通知、Singleでの移動維持、保存shiftの採択規則。
- composition revisionでunit/nav/geometryが失効、B sibling不変、blankにsource/readiness無し。
- trim 左右別/page override/blank slot/補助/RTL、capture/export targetが実Navigationだけ。
- B実削除直後にunit/anchor/resume/書き出しをF5前に検査し、再読込後もMissing参照がDBへ残る。
  個別改名/親改名/外部変更/種類設定変更の再解決と整数slot失効を検証。
- Resultでは明示Pair相方が離れていても呼び寄せない、他本と自動pair/video境界は従来builderと一致。
  hidden行はmarker/拒否、navから除外。構成本を含むResultの全shift拒否は採択回答どおり。
- 構成本/未構成本/動画/音声が混在しても結果順・media navを維持。Book詳細は構成順lock、
  Result詳細sortは従来順。OFF/混在→再適格のrecipe往復。「本を開く」案内はBookへ移れる場合だけ。
- 小編集: 方向鏡映でidentity順/Pair不変、表紙ONで当該Pairだけ単独化、OFFで補助解除、同値再送no-op、
  全非表示/Missing cover/白紙保持、新tail cover、PC/Remote同値。page overrideの左/右別値もslotで一致。
- 周辺小編集: 末尾補助On/Off、先頭/末尾/旧一括配置、表紙次/最終単独On/Offと両KeyAction。
  他Pair不変・局所単独化の順/slot・白紙/補助の中央解除・同値/対象外no-op、FollowGlobal一回解決を検証。
  1/2/多数ページ、LTR/RTL、coverなし/Hidden/Missing/途中補助、facet/切詰めに依存しない対象、Single/Split往復。

### G4: modal editor / 保存reload / multi-window

所有: dialog、operation admission、context registry integration、snapshot。
受入:

- cancel無変更、draft undo/redo、保存後identity reanchor、現在ページをhiddenにした時の採択済移動規則。
- Preparing/Editing/Saving/Reloading各状態でopen/history/F12/gamepad/Remote acquire/terminal closeのhandler試験。
- 外部manifest変更は保存拒否、DB failureはdraft保持、旧worker replyを新ownerへ採用しない。
- same-book active/parked/F12 forkと保持projectionだけ更新、別book/別prefix不変、return route・scale/anchor保持。
- editor開始→BeginAcquire / BeginAcquire→開始を決定的handler試験で入れ替える。
  Preparing/Editing/Saving/Reloading全phase、prepare reply後着、保存受付と取得の順序、
  mainだけのeditor/fullscreenなし、A編集中にB一時mountを含め、Remote拒否なし・ownerなし・viewport閉鎖を証明。
  Saving中はcommit成功/失敗の一回の終端前にFinishAcquireしない。owner終了/取得abort後のreplyで復活なし。
- ★固定解除/固定一覧帰還/F12元一覧復元/履歴の保持列はrevision不一致なら再adoption。
  新作成/変更/解除/実削除後、membershipとreturn routeは保持、旧hidden/旧順復活なし。
  同じ本の兄弟anchorがHidden→旧各reader順の次/前/新列先頭/空、Result動画/音声anchorは維持。
- 同じ本のAが20→21、Bが80→81へ着地するfixtureで、A→B/B→Aのreload完了順を反転。
  save owner=Aなら共有resumeは両順で21、一要求最大一enqueue。Bがactive/parked/後から固定解除/F12元一覧復帰でも
  81を記録しない。元窓が一覧のみなら捕捉済既存位置、位置なし/nested対象外/全非表示ならKeep。
  commit失敗はゼロwrite、Remote取得がSavingを閉じても同じ一intent。背景import/小編集で位置を奪わない。
- snapshot: light/dark、小画面/倍率、長名/長文、多ページ、button到達、IME helperの既存headless検査。

### G5: Remote / 出力 / 最終gateと文書

所有: ContainerEngine・IPC・Web、export/製本入口、関連docs/manual。
受入:

- PC/Remote entries/PageGroup/anchor/placement parity、response budget途中Pair、hidden refusal、single強制。
- strict protocol/asset token一致、session acquire refresh、必要なremote writeのgeneration失効。
- 選択export/本全冊追加/外部受け渡しでorderとhidden除外、white/supplement source無し、製本先に構成無し。
- source/cache移動・再変換・content restore・metadata importから同じ結果、通常未構成本の回帰なし。
- Remote既存SetSpreadを小編集serviceへつなぎ、表紙/方向の可視効果・commit後group cache失効・失敗通知。
  shift禁止とSingle/Split移動維持を区別。PC draft/Remote write同居なし。
  Single/Split要求でcoverを解除せず、Spreadへ戻ると保存coverを使う。
- SetFinalCover/SetSingleton (endpointと旧一括)/SetPageAloneを§9.2の共通commandへ接続。
  PC HUD/KeyActionとのrecord/unit同値、構成なしの従来spread.db、Composedのspread.db無書込、
  commit後cache失効と理由付き対象外応答、共有resume Keepを検証する。

各段階の実装者が狭いunit/handler/integration/snapshot検証を所有し、レビュー担当は有効な結果を再利用する。
共有変更の最終gateは `scripts/test-full.ps1`、fmt check、UI文字変更はglyph checker 0件。
runtime変更完成時は `scripts/build-dev.ps1`で利用者用binaryを用意する。
今回は設計だけなのでbuild/test/app launchは不要・未実施。
将来のinteractive確認は別の事前承認suiteで、対象/所要時間/desktop入力/使い捨てdataを提示する。
agentはnormal profile binaryを起動しない。承認済agent smokeはprepare-portable-smokeのcopyのみ。
利用者による通常profile確認はZIP/PDF/画像本の編集→再入場、F12両窓、Remote、白紙/trim、
Eの指定scaleを具体的に依頼し、実profile変更とsingle-instanceの注意を添える。

実装時更新: architecture/virtual-folders/display-pipeline/web-remote、keymap、metadata transfer文書、
spec、manual本/見開き/設定/Remote、保存先・外部通信説明、docs索引、detached §11。
本設計turnは本書とhandoffだけを変更する。

### ライン Eとの再base接点

利用者指示: `mimageviewer-nextview`の§1.342指定scaleはG実装より先にmasterへ入る。
その未統合diffは本worktreeにないため、Eの新型/行番号を本書で推測しない。

- `src/ui_fullscreen.rs`: `SpreadDisplayUnit/SpreadDisplayComposition`、unit anchor、
  `build_spread_display_units_for_nav`、`spread_display_composition_for_anchor`、
  Single/Spread/continuous layout、holdover/captureの基準geometry、HUD「…」の設定。
- `src/app.rs`: fullscreen anchor/NavigationDisplayDemand、items install世代、
  scale状態のcontext ownershipと保存/復元、page-edit pivot、spread設定適用。
- 関連registry: Eが追加するscale/anchor bundle項目をGのreloadで落とさない。
  geometryの基準ページはEを正本にし、Gはunit membership/role/slotを渡す。
- §1.364: E側shift保存scope/identity、HUD表示、LayoutSource contractはG/Eで一度合意して共有。
  Gは構成あり判定とeditor/initialization、Eは構成なしshift永続化を所有する。

G1以降のbriefはmerge済masterへrebase後に更新し、Eの指定scale既存テストをそのまま回帰gateへ含める。
旧anchorやfit算式をG側に複製してconflictを解消しない。

### R2指摘との対応 (検収用)

| 指摘 | 構造変更 | 主な受入段階 |
| --- | --- | --- |
| P1-1 editorと取得 | §8.1のmount非依存owner・既存session境界、§10.2の全窓close/barrier。取得拒否案は利用者代案 | G0/G4/G5 |
| P1-2 in-place items | §3.2の共通更新・整数slot remap/identity再解決。Missing recordは保持 | G1/G3 |
| P2-3 集約/mixed | §3.1のBook/Result recipe、全列一layout、sort/nav/shiftの同じ資格owner | G0/G1/G3 |
| P2-4 退避/兄弟hidden | §10.1の依存と復帰adoption、§6の全context着地 | G3/G4 |
| P2-5 転送origin | §7.3の構成専用logical family、空/不在/coverage契約 | G2/G5 |
| P2-6 folder移動/ledger | §4.1のFolder非対応の明示とbundle推奨、§7.2の保存成功Edit観測 | G0/G2/G5 |
| P2-7 Remote切替 | §9.1の可視効果ある小編集、§10.2の既存SetSpread維持案 | G0/G3/G5 |

以下の回答と独立再レビューが揃うまでdetached構造合意は未完了。

### R3指摘との対応 (検収用)

| 指摘 | 改訂 | 受入 |
| --- | --- | --- |
| P2-1 共有resume | §6.1で保存要求のowner/seedを一度捕捉し、要求完了ownerのみ記録。窓の再着地/退避復帰は読書writeなし。Q5を補足 | G3/G4、A/B完了順反転 |
| P2-2 周辺見開き操作 | §9.2でHUD/二つのKeyAction/Remote三系統を同じrecordの明示commandへ。Q17で例外と代案を相談 | G2/G3/G5 |
| P3-1 音声と案内 | §3.1/Q13/Q16へ音声を含め、Bookへ移れない対象には再openを構成閲覧の案内にしない | G1/G3 |
| fixture依存順 | G1は構成なし境界、G2でpure model fixture、G3で構成ありintegration | G0で順序確定 |

## 14. 利用者への未決質問 (推奨は全て未承認)

初稿の1〜8を維持して具体化した。6に同居していた本扱いOFF/混在/Remoteの判断は12/13/11へ分けた。
9以降はレビューによる追加判断。R3で5/6/13/16を補足し、17を追加した。
どの選択も回答前に実装仕様として確定しない。

1. **構成を作る前に記憶した「1ページずらし」は残しますか。**
   推奨: 今の組を構成の初期値に取り込み、以前のずらし記録も残す。構成がある間は使わず、
   構成を解除した時に「ずらしを記憶」がONで元のページを確認できれば再び使う。
   OFFなら使わず、記録したページが見つからない時も別ページにずらさない。解除前に結果を説明する。
   元の設定を失わずに済むため推奨する。
   代案: 構成を作ったら古いずらし記録を消す。構成解除後は基本の組から読む。

2. **PCの右開き/左開き・表紙あり/なしは、今までのメニューで直接変えますか。**
   推奨: 直接変更できる。開く方向は左右を入れ替えるだけで、指定した組を保つ。
   表紙ありへ切り替えると先頭の表示可能なページが単独になり、組だった相方も単独になる。
   他の組は変えない。表紙なしへ戻すと表紙は中央の単ページになり、補助としての表紙表示を外す。
   以前の相方とは自動で組に戻さない。この表紙操作だけは編集画面外で組を解く例外になるので承認を求める。
   操作の手間を増やさず、切替が画面に反映されるため推奨する。
   代案: 方向/表紙のメニューで編集画面を開き、変更を見て保存する。即切替には一手間増える。
   単ページ/見開き/横長分割の表示切替はどちらでも今までどおり。

3. **後からページを追加した時、既存の組と補助の表紙をどうしますか。**
   推奨: 新ページは元の名前順で末尾に中央の単ページとして足し、既存の組は変えない。
   補助の表紙は指定した単ページに残す。途中へ残る場合も「末尾表紙」と呼ばず
   「表紙の補助表示」と説明する。意図した組が勝手に変わらず、追加直後から同じ並びで読めるため推奨する。
   代案: 新ページ同士だけ自動で組む、または補助表紙を新しい末尾へ自動移動する。
   この二つは別々に選べるが、どちらも追加時に表示が自動変更される。

4. **白紙だけが続く指定を残しますか。**
   推奨: 編集画面には残すが、一覧・単ページ表示・ページ送り・書き出しには出さない。
   実ページと組んだ白紙だけ閲覧時に見える。区切りとしての指定を失わず、白紙だけの送りを増やさないため推奨する。
   代案: 実ページと組まない白紙を保存できなくする。連続白紙などの編集は制限される。

5. **保存によって読んでいるページが非表示になった窓は、どこへ移しますか。**
   推奨: 変更前のその窓の読み順で次にあった、今も表示できるページへ移す。
   なければ前のページ、それもなければ新しい一覧の先頭、全て非表示なら空の一覧に戻す。
   編集した窓だけでなく、同じ本の別窓や後で戻る固定一覧にも同じ規則を使う。
   次に読む場所を予測しやすいため推奨する。保存前に編集元窓の行き先と他の窓への影響を示す。
   次回の再開位置は、保存を押した窓がその本で読んでいた位置を基準に一度だけ更新する。
   その窓が本を読んでいなければ、以前に記録した位置を基準にする。各窓の移動や戻り用一覧の更新では上書きしない。
   どの窓の読み直しが先に終わっても次回の再開位置が変わらないため推奨する。
   代案: 非表示になった窓は常に閲覧を閉じ、一覧から選び直す。
   しおりなどから非表示ページを新たに開こうとした時は、どちらでも通知して拒否する。

6. **今回、EPUBにも構成を作れるようにしますか。**
   推奨: 今回はZIP等の対応書庫・PDF・画像本フォルダに限定し、EPUB追加は後で検討する。
   既存のEPUB閲覧は維持する。変換後ページと元本の対応を追加検討する範囲を減らせるため推奨する。
   代案: EPUBも今回対象にする。その変換・再変換時のページ対応を別途設計してから実装する。
   構成全体の編集画面を今回はPC限定にする案も、この問の明示的な採択対象とする。
   Remoteにも全体編集を追加する代案は、別の編集画面と操作設計が必要になる。
   Remoteの既存の方向/表紙切替は11、周辺操作は17で別に判断する。

7. **めったに起きない保存失敗や構成データの読込失敗は、どこまで自動回復させますか。**
   推奨: 保存失敗なら以前の構成を残し、編集画面が開いている間は未保存の編集を残して通知する。
   利用者が保存し直すかキャンセルする。保存後の読み直しだけ失敗したら保存済み構成を残し、
   再読込/再起動を案内する。自動の繰り返し保存や終了後の未保存編集の復元は作らない。
   Remoteへの切替や窓を閉じた後は未保存編集を残さない (9の選択)。
   構成を読めない場合はデータを消さず書込を止め、通知する。
   明示的に「今回は構成を使わず開く」を選んだ時だけ、その窓のその本を通常の並びで開く。
   他の窓/本には適用せず、次回は再度確認する。元データは保持し、ずらし・構成編集は止める。
   読めない構成を勝手に作り直して利用者の指定を失わないため推奨する。
   代案: 未保存編集の自動退避や複数世代からの自動回復を追加する。設計・検証の範囲が増える。

8. **白紙と組んだ実ページには、どちらのトリム設定を使いますか。**
   推奨: 構成による見開き表示では、画面上で実ページがある左/右の設定を使う。
   単ページ表示では単ページ用の設定を使う。ページごとの指定がある場合も、従来の優先順位でその側を使う。
   表示される側と設定が一致するため推奨する。構成を使っていない本の単独ページの挙動は変更しない。
   代案: 白紙と組んだページも常に単ページ用の設定を使う。左右別の余白指定はそこで効かなくなる。

9. **PCで構成を編集中にRemoteから操作を始めたら、どちらを優先しますか。**
   推奨: 今までどおり後から始めたRemoteを優先し、PCの閲覧窓と編集画面を閉じる。
   未保存の変更は破棄してPCへ通知する。「保存」を既に押して送信した処理は一度完了させ、
   成功ならその構成、失敗なら以前の構成をRemoteへ渡して結果を通知する。未保存編集を自動復元しない。
   取得途中にPCで編集を開こうとしても開かない。既存の操作切替を維持し、編集中の二重操作をなくすため推奨する。
   代案: 編集が終わるまでRemoteの取得を断る。未保存編集は守れるが、今までの「後からの操作が優先」を変える。

10. **画像本フォルダをアプリの外で移動したりドライブ文字を変えた時、構成をどう引き継ぎますか。**
    推奨: 勝手には引き継がず、移動前に書き出したメタ情報を新しい場所へ読み込む。
    古い構成は残す。書き出していなければ旧場所へ戻して書き出す手順が必要になる。
    PDF/書庫は既存の内容一致による復元を使えるが、画像フォルダは今の仕組みでは識別できない。
    同名の別の本へ誤適用せず、フォルダ専用の自動識別を増やさないため推奨する。
    アプリ内の対応する名前変更/移動では構成も引き継ぐ。
    代案: 「旧場所の構成を選んで引き継ぐ」画面を追加する。移動後でも選べるが、別の本を選ばない確認と追加実装が必要。

11. **Remoteでも、構成がある本の方向/表紙切替を残しますか。**
    推奨: 残す。2と同じ変更を保存し、右/左開きは左右を入れ替え、表紙切替はそのページの組だけを解く。
    指定済みの他の組をずらさず、表紙なしへ戻しても自動で組に戻さない。
    順番や任意の組の編集はPCの編集画面で行う。既存のRemote操作を失わず、PCと同じ結果になるため推奨する。
    代案: 構成がある本ではRemoteの方向/表紙変更を制限し、PCで変更する案内を出す。
    これは今使える切替が使えなくなる変更なので承認が必要。2で編集画面を使う案を選んだ場合も、Remote側は別途この判断が必要。

12. **画像フォルダの「本として扱う」をOFFにしたら、保存構成も一時的に使わなくしますか。**
    推奨: 実際に通常フォルダ扱いになった時は通常の並び・自動の見開きへ戻す。
    構成は消さず、非表示ページは開けないまま。本扱いを戻したら保存構成で読む。
    OFFの意味と詳細一覧の並べ替えが揃うため推奨する。明示的な本置き場の本まで、この自動設定だけで通常フォルダにはしない。
    代案: 保存構成がある限り本扱いを維持する。指定順は残るが、OFFや詳細sortがその本には効かなくなる。

13. **本のページと動画・音声が同じ一覧に混ざったら、どのように読みますか。**
    推奨: その混在一覧の通常順・自動の見開きで直接読み、動画・音声にも従来どおり進める。
    保存した構成は消さず、非表示ページは開けないまま。動画・音声がなくなり本の一覧へ戻れば保存構成を使う。
    子フォルダの行だけがある場合はページ部分へ構成を適用でき、動画・音声の混在とは区別する。
    通常の混在閲覧を維持し、動画・音声を送りから落とさないため推奨する。
    代案: 混在一覧でも構成を使う。画像の指定組と動画・音声の位置を一緒に扱う追加設計が必要。

14. **検索などの結果に構成を持つ本のページが混ざる場合、「1ページずらし」を止めてよいですか。**
    推奨: 送り対象にそのページが一枚でもあれば、その結果一覧全体のずらしを止める。
    構成のない本だけの結果なら従来どおり使える。構成なしのページも一緒にずらせなくなるが、
    構成本にはずらしを使わない方針を一貫して守れるため推奨する。
    代案: 構成なしの本の部分だけずらせるようにする。混在一覧を部分ごとに組む追加設計が必要。

15. **「構成なし」の本のメタ情報を読み込んだ時、読み込み先の構成を解除しますか。**
    推奨: 新形式で構成を転送対象に含めた場合だけ、確認画面に「構成を解除」と表示し、
    その置換を承認した時に解除する。転送した範囲外の階層は残す。
    古い形式や構成を転送対象から外した場合は保持する。書き出し元と同じ状態にでき、解除も見て選べるため推奨する。
    代案: 「構成なし」を読み込んでも常に既存構成を保持する。安全だが同じ状態に揃えるには別途解除が必要。

16. **検索・★固定・コレクションの結果では、その結果の順で直接読み続けますか。**
    推奨: 今までの結果順・自動の見開きで直接読む。離れた位置の指定相方を呼び寄せない。
    非表示ページの記録は結果に残し、「本の構成で非表示」と示して画像内容は出さず、開こうとすると通知する。
    送りではそのページを飛ばす。保存した順・組で読みたい時は、本の一覧へ移れる対象だけ任意で本を開く。
    本扱いOFF/動画・音声の混在が続く元フォルダでは、開き直しても通常表示なので、この案内は出さない。
    直接閲覧と動画・音声を含む結果順を維持できるため推奨する。
    代案: 結果をクリックしたら必ず本を開き直す。構成で読めるが、今までの結果内閲覧と送り順を変えるので承認が必要。
    本扱いOFF/混在で本の一覧へ移れない場合は、12/13の選択も含めてこの代案を再設計する。

17. **構成がある本でも、末尾表紙・端の配置・単独表示を今までのメニューやキー、Remoteで変更しますか。**
    推奨: 変更できる。末尾表紙を付ける時や単独表示にする時は、対象ページの組だけを二つの単ページに分け、
    他の組は変えない。末尾表紙には表示できる指定表紙が必要。端の配置は既に単ページのものだけに作用する。
    中央へ戻したり単独指定を外した時は、白い相方や補助表紙を外すが、元の組へ自動では戻さない。
    「全体設定に従う」を選んだ時も、その時の値を一度取り込み、その後の全体設定変更では組を変えない。
    PC・キー・Remoteで同じ指定を保存し、単ページ/分割表示中に変えた指定は見開きへ戻ると反映する。
    今使える入口を残し、手動で決めた他の組を守るため推奨する。2/11と同じ、編集画面外の小変更としての例外になる。
    代案: PCでは編集画面を開いて変更・保存する。Remoteではこれらの変更を制限してPCへ案内する。
    変更内容を見て決められるが、PCでは一手間増え、Remoteでは現在使える操作が減るので承認が必要。
