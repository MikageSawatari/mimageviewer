# §1.242 本の構成 — 設計案

更新: 2026-10-10。ライン G / `next-book`、調査基準 `46318769107b5f86e1cd63a4fc85c33e7d061fa4`。
状態: 実装担当による設計・前提検証。設計担当の採択、利用者回答、独立設計レビューは未実施。
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
4. Remoteはローカル閲覧と排他 (`docs/web-remote-plan.md` §2.2)。構成編集とRemoteの同時変更を
   常態として設計する必要はない。取得途中のeditor終了は別途admissionで止める (§10)。

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
| 画像本フォルダ | production folder listingから本projectionへ。auto本扱いOFFでも既存構成がある対象は構成順/非表示を保つ案 (§14) |
| PDFと共通Document経路 | page manifest/adoptionから同ownerへ。password/lease/cached adoptionの既存契約を保持 |
| しおり | entry検索はscopeとidentityのみ返す。item_index hintの生成をやめ、構成後のtarget resolverで解決 |
| Snapshot leaf / required open / delayed completion | 同じtarget resolver。保存したraw列や直接leaf一致だけで構成を迂回しない |
| Remote ZIP列挙とpage context、PDF、folder listing | 同pure ownerをcore workerから使用。Web側に並べ替え・pairingの第二実装を置かない |
| 戻り用保存一覧 / F12 fork / parked context | 完成PreparedBookのimmutable projectionをbundleと共に保持。適用し直して二重並べ替えしない |
| 検索・★・snapshot等の集約leaf open | 結果一覧の検索順は維持。open前にそのページのscopeの構成visibilityを解決する |

親containerの代表サムネ・pin、類似索引の元内容、メタ情報エクスポートは「本の可視ページ一覧」と別。
構成で非表示にしても既存pinや画像編集記録を破棄しない。代表画像まで変える要求は本項にないため維持する。

## 4. データモデル — 実ページと見せ方を分ける

### 4.1 BookScope / ページidentity

`BookScope { logical_root, level_prefix }` を使用する。rootは実フォルダ/PDF/元書庫、
ZIP prefixは閲覧と同じcollapse後の実効prefix。通常ZIP、nested ZIP、ZIP内directory、直接RAR、
cache閲覧で同じscope resolverを使う。階層間のfallback継承は**しない**。
root構成を子階層へ適用すると、本の別ページをhidden/missing扱いするためである。

永続キーは既存 `spread_db::container_key` と同じroot+実効prefix合成を
`path_key::normalize_keep_drive` で正規化する案。`StoreKeyNormalization::KeepDrive`を登録する。
DriveStrippedなspread/resumeと異なり、利用者が作った構成を別ドライブの同名本へ誤適用しないことを優先。
ドライブレター変更は既存content-identity restoreで対応する。root aliasは独自に増やさず、
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
flat列はunitを展開して得る派生値。実ページの通常occurrenceは構成全体でちょうど一つ、
hiddenとの重複なし。白紙と実ページを組んだunitはSingle、二実ページだけPair。
余分なblank単体/連続blankは `BlankBoundary` として編集では保持し、閲覧unitへは投影しない。
これにより「任意位置」の白紙を保存でき、白紙だけのページ送りや白紙だけのSingleを作らない。
末尾補助の表紙だけは `FrontCoverSupplement` の再利用を許す。普通のページ複製機能は範囲外。

保存検証では、種類一致、安全なrelative/entry、scope内member、重複、hiddenとの排他、
companion/placement整合、補助が指定coverを参照すること、ID一意、件数/文字列/JSON bytes上限を確認する。
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
自動で新しい末尾へ移すかは§14で利用者判断を求める。操作後の「末尾表紙」の名称/UIにも合わせる。

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

一時facet/検索欄の結果内では順を構成順に保ち、既存ペアの片側欠落を上記投影で扱う。
集約検索の別本同士を構成Pairにしない。構成を持つ各leafのvisibilityはopen admissionで評価する。

### 5.2 キャッシュのキーと失効

PreparedBookは `(scope, source_manifest_revision, policy, composition_revision)` を持つimmutable snapshot。
構成変更は既存installで必ずitems generationを進め、index/memoを同時に作り直す。
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
   保存成功後、そのidentityを既存writerで記録する。既存identityがあるならそれを優先。
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

保存した現在ページをdraftで非表示にした場合だけは、利用者の明示編集の結果なので、
保存後は次の可視実ページ→前→空一覧へ移る案。保存直前にプレビューで移動先を示す (§14)。

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

保存失敗はtransactionで旧recordが残る。draftを表示したまま「本の構成を保存できませんでした」を通知し、
利用者の再操作かキャンセルへ戻す。自動retry、途中unit保存、独自quarantine/複数段回復を追加しない。
破損/未知schemaならDBを作り直さず、通知・書込停止・構成本の閲覧拒否とする案 (§14)。
構成は利用者作成データなので、再生成可能なサムネcacheと同じ扱いにはできない。
稀なcommit後reload失敗は保存済recordを保持、通知して既存再読込または再起動を案内する。
表示を新構成で完成したと偽らず、失効した旧indexで編集/移動を続けない。
これ以上の回復が必要というレビュー指摘は、先に利用者へ割り切り案と費用を相談する。

### 7.2 登録先チェックリスト

| 接続先 | 必要な処理 |
| --- | --- |
| `rename_key_migration::STORES` | KeepDrive/uniqueな本行を登録し、exact/`/`/`::`prefix rename/copy/probe/purgeの対象へ。DB未作成は正常不在 |
| 画像個別改名/移動 | しおりの`migrate_paths_at`同様、該当containerのunits/hidden/cover内のrelative PageRefをまとめてtransaction更新。page移動がscope外ならMissingを保持。root改名だけではJSON内変更不要 |
| archive内entry変更 | 本体から行う対応操作があれば同じPageRef移行を呼ぶ。外部rewriteはMissing保持で手動再対応。ページ番号から別entryを推測しない |
| 実ファイル削除purge | 本/rootの削除成功は構成行/prefixをpurge。ページ一枚削除では親構成をpurgeせず参照をMissingとして保持。非表示操作はFS/purgeを一切呼ばない |
| コンテナcopy / 内容ID復元 | generic `copy_stores_at`の本行とvirtual-prefix mappingへ参加 (`content_identity/restore.rs:414`)。relative payloadなのでcache mapping不要。destination既存構成を上書きしない既存conflict規則を使用 |
| restorable probe / metadata cleanup | 構成も復元可能なuser dataとして数える。cleanupラベル、DB件数/clear、設定完全reset、data-dir移動・保存先説明へ登録。resetは既存明示確認の範囲で行う |
| 明示metadata export/import | PortableContainerState/nested stateへversion付き構成追加。root/prefixはbundle相対、page refsは本相対。is_empty/has_view_state/summary/import changed flagsも更新 |
| metadata import refresh | owner/context/policyを捕捉し、関係する同一本scopeのprojectionを既存reloadで更新。別本のindex/cacheを捨てない |
| 自動sidecar | 今回は構成を追加しない。既存自動mimageviewer.datはページ編集等の用途。明示bundleが構成を運ぶ |
| F12 | registryのnew/default/destructure/swap/mount/fork/park/drop、親一覧/固定snapshotへprojectionを運ぶ。cache ownerはcontextごとに独立 |
| Remote | core engineにread snapshot/lookup登録、scope resolver・list/page context・visibility admission・payload登録。IPC版とWeb asset tokenを同時更新 |

構成保存/解除、rename/import/restoreが実際に変えたscopeだけに変更通知を出す。
読み取り専用open/closeはmutation-style invalidationを出さない。
content identityは元メディアの内容であり、構成revisionを内容hashへ混ぜない。
純粋な本読みやmissing検出で永続構成を勝手に掃除しない。

metadata出力版は実装統合時の最新版+1 (調査時はv9→v10候補)。manifest/shard両方を更新し、
v7/v8/v9入力と旧字段変換を保持する。旧bundleの構成不在は「既存構成の解除」ではない。
importは既存preview/競合方針に従い、新payload全体を検証してから本行単位で書く。
imported JSONのabsolute path/`..`/unsafe entry/過大payloadは拒否し、既存データを変更しない。

## 8. エディターUI — モーダルを採用する案

本全体のorder/indexを確定する短い操作であり、数十分のダウンロード管理と異なるため、
準備→編集→保存→reloadをモーダルにする。閲覧プレビューはdraftの専用領域で行い、
背景items/fullscreen stateは編集しない。常時モデルレスでは複数本/複数窓との割り込みが増えるため採用しない。

入口は本のHUD「…」と本一覧の右クリック「本の構成を編集」。空/全非表示本でも入口を残す。
状態は一つの `CompositionEditorState = Preparing | Editing(draft) | Saving | Reloading`。
owner context/scope/requestは状態内に持ち、show bool + pending bool + draft Optionを重ねない。
共通modal helperとopen admissionへ同じ「現在画面を描くか」の述語を登録する
(`src/app.rs:21741`, `:21756`, `:37693`)。

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
背面wheel/key、slideshow、folder history、開く、F12、context activate、別editor、Remote acquire、
ファイル操作/metadata import/settings変更をadmissionで止める。OSの別アプリ操作は制限しない。
OSで兄弟窓がfocusを得ても編集owner bundleを別contextへswapしない。modal所在を案内する。
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

## 9. §1.364との境界

E側と共有する問い合わせは `resolve_book_layout_source(scope)`。
構成が存在すれば Composed、なければ Automatic + §1.364の解決済shift。
構成snapshotの取得に失敗した状態を「構成なし」としてshiftを適用しない。

`queue_spread_shift_action`が見開きずらしを作る前にLayoutSourceを見る。
Composedならactionを消費、anchor/target/cache/DBを変更せず、
「この本の見開きは本の構成で指定されています。組を変えるには構成を編集してください」と通知する。
四つのFsSpreadShift Action、カスタムキー、ゲームパッド/リング等で同じ結果にする。
Single/Splitの従来page移動はshiftではないので維持。
HUDの§1.364保存状態/解除は構成ありでは表示せず、「本の構成を編集」へ接続する。

**保存済shiftがある本へ構成を作る場合は利用者判断が必要**。
推奨案: 編集開始時の実効shiftを初期Pair/Singleへ取り込み、構成保存後はsaved shiftを休眠させる。
saved row自体を消さないので、保存跨ぎの二DB transaction/recoveryを必要としない。
構成解除時に従来のsaved shiftを再び有効にすることを解除確認で説明する。
作成キャンセルではshiftに一切触れない。記憶OFFのsession shiftも初期値に取り込むが永続shiftを新規作成しない。
「初期値に取り込んでsaved shiftを消し、解除後は基本見開き」の案も可能だが、別storeの消去失敗時の
扱いをE/Gで合意する必要がある。採択前にこの二DB処理を作らない (§14-1)。

構成のbinding設定の切替は2026-10-10追補で禁止されていない。
Single/Spread/Split切替は閲覧方法として維持し、右/左開きや表紙設定の永続ownerを二つ作らない。
構成ありのHUD方向/表紙切替を構成編集へ接続するか、同じ構成recordへの小編集として即保存するかは
未決定 (§14-2)。後者でもPairを自動再計算することはしない。既存spread.dbだけを書いて無効な操作に見せない。

## 10. F12 / 多窓 / Remote

### 10.1 ローカルcontext

DBサービスはApp共通、`PreparedBook/layout/identity map/revision/unit cache/editor draft`はcontext所有。
F12 forkはimmutable構成snapshotを共有できるが、nav/cache/queue/cancel/generationは複製したcontextのowner。
fork/park/mount/deposit/close自体はDB writeや sibling reloadを発生させない。
保存replyはrequestを発行したcontextへ配送し、今mountされているcontextを要求元と推測しない。

save後は同じscopeを表示するcontextだけを既存本reloadへ載せる。
各contextのanchor PageIdentity、元閲覧mode、filter、return route、zoom/scale設定を捕捉し、
新items投影へ個別に復元する。parked contextも既存metadata import refresh同様のcontext identityで扱う。
別本/別階層は不変。同じscopeの旧snapshotは新構成を完成表示と扱わず、既存holdoverとして使う。
窓の無条件閉鎖・zoom全resetで解決するとreleased操作を削るので採用しない。
reloadが既存routeでは状態を保存できないと判明したら、実装を拡げる前に設計担当/利用者へ相談する。

editor中のcontext terminal closeはPreparing/Editingを取消、SavingはDB replyを収穫してから終了、
Reloadingは採用/closeの既存終端へ接続する。indexを使うdraftを別窓に移植しない。
detached述語/viewportに触る前にはClaudeCodeと独立Codexレビューで
「context投影の所有を拡張する構造的修正であり症状パッチでない」ことに合意し、
実装範囲と理由を `detached-rework-plan.md` §11へ記録する。
新規detached bool、geometry捕捉、repaint/retry/time窓による修復は不要。

### 10.2 Remote

coreのContainerEngine workerが同PreparedBookを生成する。Webは返されたentries/PageGroupを表示し、
構成のorder/pairを自前で再現しない。実addressは元root+entry/物理PDF番号のまま。
`has_composition`、revision/accepted layout identity、構成の実効binding、shift availabilityをpayloadへ追加し、
既存presentation/anchor/singleton placementを共有する。白紙のaddressを発明しない。
件数/response budgetで切詰める前にfull projectionを作り、切詰め後はcomplete=falseとする。
Pairの片側が応答外なら残る実ページのslotを保ち、見かけの末尾へ表紙を付け足さない。

構成editorは今回はPCのみを推奨。Remoteは構成で読む、非表示を拒否する、binding切替の契約を満たす。
Remote編集UI/APIの追加は別段階/要求とし、本機能のPC編集と同時に作らない。
書込は必ずIPC→core所有service。read-only connection/workerから書かない。
editor中のRemote acquireは既存共通admissionで「本の構成を編集中です」と拒否し、
編集draftを突然破棄して全窓を閉じない。Local/Remote排他が変わらないことをreviewする。

PC変更の端末反映は既存session acquire時のcache破棄で扱う (`web-remote-plan.md` §2.2/§12.16)。
構成編集をPC限定にする限り、home変更検知のための新data_version watcherは要らない。
Remoteセッション内のbinding変更を許す場合は既存remote generation/invalidation通知へ登録し、
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
| 編集中に別本open / 戻る・進む / F12 / sibling activate / Remote取得 | modal admission。元に戻す/supersede draft/resume機構を作らない |
| itemsと別の読み順だけ構成順 | 本列生成ownerで一回投影。通常ナビ・単ページ・seek・連結を同じitemsへ |
| PC列挙とRemote列挙の異なるorder/hidden/pair | same PreparedBook。Web独自pairingを増やさない |
| 構成 + 一時shift + saved shiftの合成 | typed LayoutSource二択。Composedでshiftを解決/書込しない |
| 任意page/blank列 + explicit pair overrides + singleton flags | explicit units一つ。flat列は派生、blankはsourceではない |
| 自動landscape/page_alone設定変更 + 保存済手動Pair | 初期値で自動規則を凍結。以後はgeometryだけ更新 |
| 閲覧mode/orientation/Splitが本の永続ペアを変更 | view projectionを分離。single/splitは同じ可視flat列 |
| 保存中のitems in-place並べ替え + index編集cache | draft→commit→既存reload。世代とidentity表を一緒に採用 |
| 個別本reloadとF12独自rebuildを別々に実装 | 既存context付きreloadへ接続。scopeに関係するcontextだけ更新 |
| 読取open/closeでmutation invalidation | readはrevision採用だけ。実際のwrite/rename/importだけ通知 |
| 書庫cache keyと本のidentityの混在 | logical/physical typed snapshot。ページ編集DBは既存keyのまま |
| 新storeの移行/旧設計試作データ互換 | 未リリースstoreは新schema一つ。released既存DB/bundle互換は維持 |
| 稀なDB failureのretry/途中保存/rollback後のrollback | record一transaction + draft維持/通知。独自回復網は利用者相談前に作らない |

採用しなかった簡略化: 全窓を閉じる、全設定reset、PDF即時表示を全件待ちへ変える、
構成なし本のshiftを取り除く、読み取り時の新tail自動保存。いずれもreleased機能の退行、
利用者データ変更、または別contextへの不要な副作用がある。

## 13. 段階・担当範囲・受入テスト

設計担当が下記chunkを確定し、各chunkのwriterを一人にする。実装と独立レビューは別context。
本書では追加subagent/レビュー実行をしていない。設計handoff後にcoordinatorが独立レビューを手配する。
ライン E の§1.342統合前にはproduct実装を始めない。§1.364と契約が未決のままshift storeを設計しない。

### G0: 設計採択・E統合後の再照合

成果: §14の回答、独立設計レビュー、scope resolver/DTO/shift責任分担、detached構造合意。
受入: 本書§2のsymbol/lineを新masterで更新、下記E接点の差分を読み、仕様の矛盾を潰す。

### G1: items列生成の統一 (構成なしでの互換を先に証明)

所有: `app/book_materialization`、ZIP/folder/PDF/しおり/snapshot/Remote producer。
受入fixture:

- 数字名/日付/評価sort、通常folder/画像本/製本、PDF、ZIP wrapper・nested・直接RAR・変換cacheで旧順と一致。
- ZIP root/enter/back/DFS/評価ZipDir/bookmark/snapshot/Remote listとpage contextが同じscope/orderを返す。
- policy before book classification、same-name除去、cached PDF早期adoption、password cancel、古いworker不採用。
- context Aのread-only open/close/forkがBのitems/generation/queue/cancel/cacheへ変更を出さない。
- visible_indices昇順、itemsとmetas長/対応、complete proofとRemote切詰めの境界。

### G2: model / store / lifecycle

所有: pure model/resolver、store service、rename/purge/content restore、metadata transfer。
受入:

- round-trip、Pair/Single/blank/補助/hidden、重複・unsafe・unknown schema・上限拒否、一transaction rollback。
- root/prefix rename、画像一枚改名、scope外移動/ページ欠落、book削除とpage削除の違い、KeepDrive衝突なし。
- 内容IDcopy後のrelative参照、destination構成を保持、cache再変換でもlogical key一定。
- 新formatとreleased v7/v8/v9入力、nested状態、旧bundleの構成不在で既存構成を消さない。
- readでDB writeなし、clear/reset/cleanup登録漏れなし、変更通知の対象scope限定。

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

### G4: modal editor / 保存reload / multi-window

所有: dialog、operation admission、context registry integration、snapshot。
受入:

- cancel無変更、draft undo/redo、保存後identity reanchor、現在ページをhiddenにした時の採択済移動規則。
- Preparing/Editing/Saving/Reloading各状態でopen/history/F12/gamepad/Remote acquire/terminal closeのhandler試験。
- 外部manifest変更は保存拒否、DB failureはdraft保持、旧worker replyを新ownerへ採用しない。
- same-book active/parked/F12 forkだけreload、別book/別prefix不変、return route・scale/anchor保持。
- snapshot: light/dark、小画面/倍率、長名/長文、多ページ、button到達、IME helperの既存headless検査。

### G5: Remote / 出力 / 最終gateと文書

所有: ContainerEngine・IPC・Web、export/製本入口、関連docs/manual。
受入:

- PC/Remote entries/PageGroup/anchor/placement parity、response budget途中Pair、hidden refusal、single強制。
- strict protocol/asset token一致、session acquire refresh、必要なremote writeのgeneration失効。
- 選択export/本全冊追加/外部受け渡しでorderとhidden除外、white/supplement source無し、製本先に構成無し。
- source/cache移動・再変換・content restore・metadata importから同じ結果、通常未構成本の回帰なし。

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

## 14. 利用者への未決質問 (各項の推奨は未承認)

1. **構成作成前の保存ずらしをどう扱うか。** 推奨: 現在の組を初期値へ取り込み、保存ずらしは休眠保持。
   構成を解除すると以前の保存ずらしが戻ることを確認文に表示する。二DB消去の失敗処理を増やさず元データも保つ。
   代案: 初期値へ取り込んだ後にずらし記録を消し、解除後は基本の組へ戻す (別store消去失敗の割り切りも要決定)。
2. **構成ありのHUD方向/表紙切替の入口。** 推奨: Single/Spread/Splitは従来どおり即切替、
   右/左開き・表紙あり/なしの変更は構成editorへ接続し、組の変化を見て保存する。
   これは現在の即時binding切替に一手間を足すため、実装前の承認が必要。
   代案: 同じ構成recordへの即保存を維持し、directionは鏡映、cover変更もexplicit Pairを自動変更しない。
3. **後から増えたページと末尾表紙。** 推奨: 新ページは元順で末尾に中央Singleとして追加し、
   組を変えるならeditorで行う。既存unitとpairは変えない。補助表紙は指定済unitに残す。
   代案: 新ページ同士だけ自動規則で組む、または補助表紙だけ新末尾へ自動移動する。
   推奨案は新ページの寸法待ち・PC/Remoteでの暫定pair差をなくし、保存後は手動という仕様にも揃う。
4. **余分な白紙だけの並び。** 推奨: 編集では保持、一覧/Single/ページ送りには出さない。
   白紙を単独で閲覧/exportする仕様は追加しない。白紙だけのunit作成を禁止する代案は、任意位置/連続白紙の編集を狭める。
5. **保存時に今読んでいるページを非表示にした場合。** 推奨: 編集プレビューで示した次の可視ページ、
   なければ前、全非表示なら空一覧へ。既存しおり/保存位置targetからのhidden移動は一律拒否のまま。
6. **画像本扱いOFF・mixed化/EPUB・Remote編集の対象。** 推奨: 構成作成は画像本/PDF/対応画像書庫に限定。
   既存構成がある画像フォルダは本扱いOFFでも構成順/非表示を保ち、後から動画/子folderが増えたら
   その非ページblockは通常順で残す。EPUBは既存閲覧を維持するが作成対象への追加は保留。
   Remoteは構成閲覧対応まで、構成editorはPCのみ。混在後に構成を無効化すると非表示が復活するので推奨しない。
7. **新storeの破損/未知版と保存後reload失敗の割り切り。** 推奨: 構成データを消さず書込停止・通知。
   読み取れない構成本は開かず、保存後reloadだけ失敗したら保存済データを保持して再読込/再起動を案内する。
   自動retry/多世代backup/段階回復は追加しない。DB全体の読込不能では存在有無も不明なので、
   対象本の構成を無視して閲覧する回避は利用者の明示選択とする (勝手なsilent fallbackはしない)。
8. **左右別トリムを白紙相方の実ページにどう当てるか。** 推奨: 見開き閲覧では実slot側のspread margin、
   Single閲覧ではsingle margin。元ページにmarginを移植しない。§1.240既存singletonの挙動変更は別途扱う。
