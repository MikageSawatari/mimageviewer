# §1.242 本の構成 — 設計案

更新: 2026-10-11、G3実装と安全でない書庫参照の扱いを追記。ライン G / `next-book`。
設計履歴: R6は独立再レビューACCEPT、P3文言修正は `330e070f6`。G0照合treeは `76488f0ec`。
Eの共有BookScopeKeyと§1.364はmerge `97c9b711d`で取り込み済み。G3の実装・検証結果は§13とhandoffへ記録し、受入とは区別する。

設計節の型名・ファイル名には提案時の表記も残る。実装済み段階の所有境界は§13を参照する。
§2とG0の現状記録は当時のソース調査であり、製品を起動して観測した記録ではない。

**2026-10-11の開発・出荷判断**: G2 part B (`249dc75a0` + `77ca29b29`) は独立レビューでACCEPT。
§1.242は次々回releaseへ送り、`next-book`でG3以降を開発する。今回masterへ統合しない。

## 1. 要件と範囲

正本は [next-release-backlog.md §1.242 / §1.364](next-release-backlog.md)。2026-09-15 の利用者決定と
2026-10-10 の追補を区別する。

- ZIP/CBZ、PDF、EPUB、画像だけの本フォルダ、直接閲覧・変換閲覧する対応書庫に、本単位の非破壊な構成を保存する。
  書庫階層ごとに独立する。製本フォルダは構成作成対象外。変換書庫の保存先 identity は元書庫。
- ページの並べ替え、非表示、任意位置の白紙、見開きの組、単ページ配置を編集する。
  表紙あり/なし・右開き/左開きを構成に含める。方向は本の作り方によって決まるため、閲覧設定ではなく構成の所有とする。
- 作成開始時の自動構成を初期値に取り込む。その後、全体設定やページ寸法の後着で手動の組を作り直さない。
  一覧も構成順とし、非表示ページは通常一覧に出さない。未登録の実ページは末尾に足す。
- 非表示ページへのしおり、検索、評価一覧、保存位置等からの移動を拒否してトーストを出す。
  ファイル自体、ページ編集、しおり等の記録は消さない。「非表示」を実ファイルの「削除」と混同しない。
- 白紙は構成だけの要素。一覧・単ページ表示・実ページ数・書き出し・製本追加には含めない。
  横長分割は本の構成に保存せず、閲覧方法として独立させる。
- **2026-10-10 決定 (R4)**: 構成がある本の組に関わる操作は、全て本の構成の編集画面だけで行う。
  表紙、右/左開き、末尾表紙、端配置、強制単独、一ページずらしをHUD・キー・mIV Remoteから変更できない。
  同等のカスタムキー/ゲームパッド等も共通ownerで拒否する。HUD/Remoteの該当項目は無効表示とし、
  「本の構成で設定されています」と説明する。HUDに構成使用中の小さな表示と「本の構成を編集」を置く。
  単ページ/見開き/横長分割の表示切替と、単ページ時の従来のページ移動は維持する。
- **Q1決定**: 初期構成は現在の表示を取り込み、以前の§1.364保存shiftは休眠保持する。
  構成解除時だけ§1.364の現時点の規則で再評価する。作成/編集で全体・既定の見開き設定を書き換えず、
  構成がある本へ全体・既定設定を適用しない。他の本の見え方を変更しない。

EPUBも今回対象。既存個別編集と同じ元EPUB + 物理ページ番号を使う (§4.1)。
再変換でページがずれる場合は既存編集と同様に構成もずれる仕様を利用者が了承した。
画像だけの本フォルダは独立したG4Fで追加する。難航時は利用者相談でその段階だけ外し、
書庫/PDF/EPUBを先に完成できる (§13)。製本フォルダは引き続き対象外。

抽出一覧A2は**v4.5.0の既存挙動を意図して変更する、2026-10-10承認済み仕様**。
Ctrl+G/タグ/評価/スマートフォルダ/コレクション/★固定/サブフォルダ展開は、開くたび単ページ。
一時見開きでは一覧全体を一冊の通常列として隣接ページを組み、元本の構成・本別設定は適用しない。
表紙/shift/末尾表紙/端配置/強制単独は使わず、modeも記憶しない。結果順とサムネイルは通常どおり。
非表示は一覧生成時に調べず、開く対象/送りで新たに表示するページだけ遅延照合する (§3.1 / §6)。
読書履歴・しおりから本自身を開く経路、通常フォルダの詳細sortはこの抽出一覧契約に含めない。

参照した原則: `CLAUDE.md` のバグ修正・状態削減・まれな失敗・退行相談・永続データ・IME・UI文字列、
[detached 憲法 §2](detached-rework-plan.md#2-憲法-全ステージ共通の不変条件禁止事項-最重要)、
[architecture-overview.md](architecture-overview.md)、[virtual-folders.md](virtual-folders.md)、
[display-pipeline.md](display-pipeline.md)、[本の読み順の旧計画](archive/folders-archives/book-page-order-plan.md)、
[§1.239/§1.240 計画](spread-endpoint-and-blank-pages-plan.md)、[web-remote-plan.md](web-remote-plan.md)、
[keymap-spec.md](keymap-spec.md)、[key-customization-impl-plan.md](key-customization-impl-plan.md)。
旧計画の「未実装」や行番号を現状の証拠として流用しない。

## 2. バックログの前提を現行コードで照合した結果

表と追加前提1〜19の全symbol/参照を、E統合後の `76488f0ec` へ再照合した。
本文の既存source参照も同treeへ更新。旧lineの単純加算ではなく、git差分の不変区間と現行symbol/周辺処理で確認した。
「成立」はコード構造の確認であり、実機での成功を意味しない。

| 9月時点の前提 | 現行の根拠 (file:line) | 判定・設計への反映 |
| --- | --- | --- |
| 本は名前順で items を作る | `src/app.rs:42` (`BOOK_READING_PAGE_ORDER`)、`:418` (`folder_media_sort_order`)、`src/app/folder_scan.rs:276`、`src/app.rs:35384` (`materialize_zip_grid`) | 成立。PDFは番号順、製本は Numeric、通常フォルダは一覧設定。旧読み順計画の未実装記載は古い |
| items 順を変えれば下流が揃う | `src/ui_fullscreen.rs:16194` (`ViewerNavigationCaches`)、`src/app.rs:56950` (`current_reading_history_page_position`)、`src/ui_fullscreen.rs:46446` (`grid_selection_indices`) | 主要な下流は追随するが、それだけで完了しない。別 producer と直接 leaf open を揃える必要がある |
| `visible_indices` は昇順 | `src/app.rs:488`、`:69169` (`binary_search`) | 成立。構成を visible_indices の任意並べ替えとして実装しない |
| 末尾補助等の適格性は nav数=items数 | `src/ui_fullscreen.rs:17016` (`spread_complete_page_permutation`) | **旧バックログから更新済み**。現行は `smart_folder_rule_total()` と資格・page data・重複なしの証明。`items.len()` 比較だけではない。構成用の complete proof を生成境界で渡す |
| ZIPの列生成が分散 | `src/app.rs:33330`, `:35369`, `:35384`, `:35425`, `:35780`, `:36142`, `:36174` | 成立。初回、物理履歴・評価ZipDir着地、階層移動、DFS、スライドショーの本跨ぎを共通化する |
| 画像フォルダとPDFにも個別列生成 | `src/app/folder_scan.rs:257`, `:276`, `:425`、`src/app.rs:36605` (`adopt_direct_pdf_rows`)、`:37491` (`poll_pdf_enumerate`) | 成立。走査/列挙のI/Oを一本にするのではなく、列挙後の本投影 owner を一本にする |
| しおり/スナップショットが階層列を再生成 | `src/book_bookmarks.rs:1525` (`resolve_archive_bookmark_target`)、`src/app.rs:47425`、`src/app/snapshot_ops.rs:2242` | 成立。しおり resolver は raw index を算出する。構成後 index と混同できない |
| Remote ZIPは arrange を通らない | `src/remote_ipc/container.rs:3963` (`enumerate_zip_page_context`)、`:4024` (`arrange_grid_items`)、`:5670` (`enumerate_zip`)、`:5729` (`materialize_level`) | **入口ごとに異なる**。page context側は arrange済み、通常列挙は materialize直後。差が出る実行観測は未確認。二つとも共通投影へ置換 |
| Remote画像フォルダも独自 | `src/remote_ipc/container.rs:5552`, `:5565`、`src/app/folder_scan.rs:257` | 既に production folder listing を再利用。新しいRemote専用folder sorterを作らない |
| 編集中は items を触らず保存後 reload | `src/app.rs:35503`, `:36622` (`prepare_visible_install`)、`src/app/viewer_context_registry.rs:1813` (`set_items_generation`) | 実装可能な所有境界。現在のindexからの直接並べ替えは不可 |
| 白紙を GridItem にしない | `src/ui_fullscreen.rs:9596` (`SpreadPair`)、`:14346` (`SpreadPageOccurrence`)、`:14510` (`compose_spread_navigation_unit`) | 成立。idxは実在ページ。§1.240の白い相方はplacementで表現済み。新規偽パス/texture/sourceを作る理由がない |
| 見開きの編集は左ページへpivot | `src/app.rs:11918` (`plan_page_edit_pivot`) | 成立。白紙をDoubleの片側にすると編集targetが壊れる。白紙相方はSingleへ投影 |
| 見開きは両textureが必要 | `src/ui_fullscreen.rs:41633`, `:41728` (左右source選択・描画)、`src/materializer.rs:43` (`MergedSpread`) | 成立。完成/通過画像はunit単位で揃える。白紙にsource readinessを要求しない |
| builderは隣接/横長/shiftのみ | `src/ui_fullscreen.rs:14941`, `:14958` | **追加あり**。現在は `PageAloneSettings` と非pairable境界・forced singleton原因も入力。既存自動builderを構成に再適用しない |
| unit/nav cacheに構成がない | `src/ui_fullscreen.rs:14619` (`SpreadDisplayUnitsToken`)、`:16215` (`ensure_source`)、`:16938` | 成立。unit tokenはitems世代/mode/shift/landscape epoch/page_alone/nav、nav sourceは世代/flow。構成revisionと列投影を同一snapshotに結ぶ |
| resumeはitems indexだけ | `src/book_resume_db.rs:35`, `:78`, `:140`, `:174`, `:437`、`src/app/file_type_resume.rs:15`, `:135`、`src/app.rs:56814`, `:57034` | **古い**。raw index + optional logical identityのtransaction保存・worker構築対応表が既にある。既存型に構成を投影し、raw-only旧記録だけ橋渡しする |
| 履歴の位置も要確認 | `src/reading_history_db.rs:67`、`src/app.rs:22335`, `:22381`, `:56950` | historyのlast_pageは1-based読書ordinal、identityではない。履歴はcontainer openを経由する。位置の正本はresumeへ寄せ、表示ordinalを再開indexに転用しない |
| rename/purge/export/content restore登録が必要 | `src/rename_key_migration.rs:939`, `:1198`, `:1299`, `:1447`, `:1523`、`src/content_identity/restore.rs:181`, `:414` | 成立。行キー移行だけではJSON内の画像相対identity改名は更新されない。generic copyと専用ページ改名の双方が必要 |
| metadata transferを版上げ | `src/metadata_transfer.rs:27`, `:453`, `:492`, `:3203`、`src/app/metadata_import_refresh.rs:534` | 現行出力v9、入力v7/v8/v9。v7だけからの更新案は古い。既存入力版を保持し、新版は統合時の最新+1 |
| F12 bundle登録 | `src/app/viewer_context_registry.rs:823`, `:829`, `:912`, `:1075`, `:2241`, `:2515`, `:3466` | 成立。items、identity index、unit cache、shiftはcontext所有。構成も同じbundleへ載せ、App globalな現在構成で描かない |
| 変換書庫のDBキーが割れている | `src/app.rs:25715` (spreadはtree物理root)、`:25772` (pinは元root)、`:46856` (しおり元root)、`src/book_resume_db.rs:50` | 成立。ただしresume logical identityは既に元rootを使う。構成のために既存page編集キーを一斉変更しない |
| spread_separateへの影響 | `src/view_trim.rs:163`, `:226`, `:378`、`src/ui_view_trim.rs:831`, `:842`、`src/ui_fullscreen.rs:41592` | sideは画面Left/Right。ページordinalの奇偶ではない。構成slotを通してsideを確定する |
| Remote縦長解除は端末設定 | `crates/remote-web/web/app.js:13199` (`portraitSinglePage`)、`src/remote_ipc/container.rs:6076` (`force_single_page`) | 成立。旧app.js:11536を置換。本体のSplit、端末single強制を構成payloadへ混ぜない |
| session shiftは再入場で消える | `src/app.rs:25885`, `:44563`、`src/ui_fullscreen.rs:28390`、`src/app/gamepad_input.rs:6550`, `:6964` | 成立。§1.364は未実装。キーだけの抑止では不十分で共通action ownerで拒否する |

調査で重要な追加前提:

1. ファイル種類policyは本判定の**前**に作用する (`src/app/folder_scan.rs:286`)。
   構成の非表示、種類除外、一時facet絞り込みを同じ「存在しない」に潰すと、解除/直接移動が壊れる。
2. ZIP階層はwrapperをcollapseする (`src/zip_tree.rs:217`, `:368`, `:413`)。
   しおりのouter+entryから、実際にページを所有する実効階層を解決してから構成を引く。
3. 深いZIP階層のresumeは現在記録しない (`src/app.rs:56824`)。
   本機能に便乗してcross-book resumeを導入しない。階層内の保存時reanchorはidentityで行う。
4. Remoteはローカル閲覧と排他 (`docs/web-remote-plan.md:51`, `:57`, `:83`)。
   **取得をeditorのUIモーダルで止められるという初稿の前提を撤回**する。
   別スレッドの `SessionStateMachine::acquire` (`src/remote_ipc/session.rs:309`) が先にBeginAcquireを確定し、
   UIは後で全窓を閉じる (`src/remote_ipc/ui.rs:1081`)。この既存契約へeditorも含める (§8.1 / §10.2)。
5. 初回列生成以外にもitemsが変わる。実削除完了 (`src/app.rs:46228`) は
   `remove_items_batch` (`:43053`, `:43101`) で直接remove・世代更新し、自動reloadを抑止する (`:46231`)。
   identity表の既存remapだけでは新設explicit unitは直らない。投影ownerの行変更契約も必要 (§3.2)。
6. 集約からの直接閲覧はreleased経路 (`src/app/snapshot_ops.rs:1737`)。
   reader orderはsnapshot/collection/detailsを尊重する (`src/app.rs:68677`)。
   navは動画も含む (`src/ui_fullscreen.rs:13632`)。本を開き直すことを強制しない (§3.1)。
7. 退避列の復帰は直接代入 (`src/app/snapshot_ops.rs:1282`, `:1406`、
   `src/app/viewer_context_registry.rs:3418`)。現在のcurrent_folderだけの変更通知では不十分 (§10.1)。
8. メタ情報転送のcontainer familyはorigin一つ (`src/metadata_transfer.rs:2436`) で、
   source/cache混在はエラー (`:2163`)。importは共通target key (`:4396`, `:4695`)。
   構成をそのfamilyへ追加せず、専用の論理scope転送契約へ分離する (§7.3)。
9. 内容IDはFolderを持たず (`src/content_identity.rs:62`)、Folder itemは対象外 (`:169`)。
   画像本フォルダの外部移動を既存内容IDが復元するという初稿の前提を撤回する (§4.1)。
   保存成功時のEdit観測も必要 (`src/app.rs:25962`、§7.2)。
10. Remoteの方向/表紙操作は既存 `SetSpread` (`src/remote_ipc/ui.rs:2322`, `:2552`)。
    mode/direction/coverを同時に保存する現行経路なので、表示切替だけを許す入口と分離が必要。
    R4の明示決定により、構成あり本の方向/表紙変更を拒否する (§9.1/9.2)。構成なしの既存操作は維持する。
11. resumeのDBはpath一つにつき一行 (`src/book_resume_db.rs:128`)。
    writer受付 `persist_book_resume_with_identity` (`src/app/book_resume_meter.rs:181`) にcontext優先順位はない。
    窓ごとの再着地から書き込むとreload完了順が共有位置を決める。保存要求の一ownerだけが記録する (§6.1)。
12. 方向/表紙以外にも、PC HUDの末尾表紙・端配置・強制単独 (`src/ui_fullscreen.rs:25466`, `:25479`, `:25491`)、
    強制単独KeyAction (`:29967`, `:29980`)、Remote同等要求 (`src/remote_ipc/ui.rs:2327`, `:2331`, `:2350`) がある。
    現行setterはspread.dbへ保存 (`src/ui_fullscreen.rs:37420`, `:37471`, `:37520`、
    `src/remote_ipc/ui.rs:2603`, `:2644`, `:2703`)。R4ではComposedに対する書込前の共通拒否へ接続する (§9.1)。
    R2/R3で提案した構成recordへの直接小編集は、利用者決定により撤回する。

R5/R6の追加前提 (G0で同treeへ再照合):

13. EPUBの `GridItem::PdfPage.pdf_path` は元EPUB。共通PDF列生成は要求された論理パスを保持する
    (`src/ui_dialogs/epub_convert.rs:661`, `src/app.rs:36762`, `:36941`, `:37242`)。個別編集キーは `page_path_key` →
    `edit_source::page_key_for_grid_item` → `page_key_for_pdf` → `zip_entry_key` で
    `normalize(元EPUB)::page_<0-based番号>` になる
    (`src/app.rs:75978`, `src/edit_source.rs:362`, `:394`, `src/adjustment_db.rs:550`, `:561`)。
    物理cache解決は `pdf_loader::resolve_read_target` (`src/pdf_loader.rs:964`) 以後だけ。
    固定ページ寸法は `crates/epub-pdf-worker/src/render.rs:12`, `:114`。
    同一入力/変換器の安定性を利用者の前提として採択し、将来の版差まで一致を保証するとは書かない。
14. 画像folderのsidecar復元は**自動**。`sidecar_restore_eligible` (`src/app/sidecar_restore.rs:1048`) と
    `ImportFamilies.edits` (`:1161`) がbackup設定に従い、ImportRequiredから確認質問なしでworkerへ
    進む (`:1988`, `:2033`)。既存編集は `ON CONFLICT DO NOTHING` (`src/sidecar_import.rs:1638`)。
    `sidecar_backup_enabled` は既定true (`src/settings.rs:7850`)。この不足行補充に構成を合わせる (§7.4)。
15. 明示転送は空も置換。page/container family削除→値があるものだけ挿入
    (`src/metadata_transfer.rs:4571`, `:4645`, `:4831`, `:4950`)。
    UIは一括確認一回で範囲・適用数・スキップ数と上書き説明
    (`src/ui_dialogs/metadata_transfer.rs:829`)。カテゴリ選択/本ごとの確認はない (§7.3)。
16. 現行抽出結果は自動見開きが利用可能で、本境界なしに隣接nav画像を組む
    (`src/ui_fullscreen.rs:13632`, `:15323`)。cover/shiftも使え、末尾等はcomplete book条件が制限する
    (`:15070`, `:17036`, `:28390`)。A2はreleased挙動の変更であり、互換テスト差分を限定して説明する。

17. sidecarの自動復元はmtime秒値の比較ではない。`SidecarDiskToken` はfolder/byte length/mtime nanos/SHA-256を捕捉し、
    `sync_marker()` が内容を混ぜた値を生成する (`src/sidecar.rs:187`, `:203`)。
    probeはmarker完全一致 (`src/sidecar_import.rs:795`)、再利用証明はpending/failed writerを先に判定し、
    全bytesの再検証も行う (`src/sidecar.rs:358`, `src/sidecar_import/probe_reuse.rs:141`, `:164`)。
    明示metadata importのmtime秒値 (`src/metadata_transfer.rs:4824`) を構成の消費記録には流用しない (§7.4.2)。
18. 現行writer/import snapshotはitemsだけで同一性を判定する (`src/sidecar.rs:463`)。
    空itemsならfile削除 (`:1055`)、flush完了はitems Arc/disabledだけを照合 (`:1148`)、
    reservation捕捉もitemsだけ (`:1314`)。root構成を加える時は全snapshot契約を拡張する (§7.4.1)。
19. 連結読みの通常navはsequenceを作らずearly returnする (`src/ui_fullscreen.rs:11420`)。
    実際の提示候補は `continuous_reading_layout` (`:38552`, 可視判定 `:38621`) が作り、
    `draw_fs_continuous_reading` (`:39161`) がtextureを選んで直接paintする (`:39367`)。
    anchor移動へのadmission追加だけでは不十分。既存continuous unit/presentation ownerへ接続する (§6.2)。

20. E §1.342はfitの入力/保持を拡張したが、unit membershipと読書anchorの正本は引き続き
    `SpreadDisplayComposition` → `fs_navigation_display_demand_for_anchor`
    (`src/ui_fullscreen.rs:17103`, `:17146`)。新しい `ContinuousSpecifiedFitReference` (`:8163`)、
    `prepare_continuous_specified_reference` (`:38245`)、`FullscreenFitInput` (`src/fullscreen_specified_scale.rs:52`)
    を構成のorder/pairing ownerへ置換しない。Eへの接続は§13のG0照合記録に具体化した。

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

**既存ZIPの参照を構成で表せない場合 (2026-10-11設計担当決定)**:
既存ZIP閲覧は`a//b.jpg`のような名前や、`page.jpg`/`PAGE.JPG`のような正規化後に衝突する名前も保持する。
既存producerに構成用の安全性検証・重複除去が済んでいると仮定しない。
構成の準備workerは、その階層のraw member全件を既存のPageDomain検証と参照の等価性規則で照合する。
不正参照・重複identityなど、構成modelでは全ページを表せない場合は**その本の構成準備だけを拒否**する。
元書庫・保存済み構成を変更せず、ページの除去・統合を行わず、構成を使わずに従来どおり開く。
新機能の適用範囲の制限であり、既存ZIP閲覧を制限する変更ではない。
通知は「この本のページ名では本の構成を使えません。構成を使わずに開きます。」とする。
同じprepareの完了時に理由と対象の本をログへ記録し、frameごとの通知・再試行を行わない。
構成の新規作成・保存も同じ検証に従い、保存済みrecordを消去・再作成しない。
このsource準備の拒否は、§7.1の読めないstore/構成の原本保護と明示ignoreの契約とは区別する。
raw scopeを通した全件検証をpolicy除外後の行だけで代用せず、曖昧な元ページからcomplete proofを作らない。

投影は次を**同じ結果**として返す:

- 実在・採用済み・構成で可視な実ページだけを構成順で置いたitems/metas。
  ZIPの子container行は既存block順を保ち、構成は当該階層の直下ページblockだけを並べる。
  階層を跨ぐページ移動/子本の隠蔽/子ZipDirの並べ替えはしない。
- 元manifest identity→結果index、構成によるHidden、Missing、種類除外の解決表。
  `PreparedPageIdentityIndex` に適合するresume表も同時に生成する。
- indexへ解決済みのexplicit unitsとpresentation、構成revision、visible-page complete proof。
  hiddenも「登録済み」と数え、後から新ページとして再追加しない。
- 元manifestにあるが構成に未登録のページを元順で末尾へ追加する派生tail。
  各ページを中央Singleとする (Q3決定)。寸法待ちや既存末尾との自動pairingを発生させない。
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
| 画像本フォルダ | production folder listingから本projectionへ。G4Fのみで接続。資格を同じownerで確定。本扱いOFF/混在はOrdinaryListとして従来表示 (§3.1) |
| PDF/EPUBと共通Document経路 | page manifest/adoptionから同ownerへ。password/lease/cached adoptionの既存契約を保持 |
| しおり | entry検索はscopeとidentityのみ返す。item_index hintの生成をやめ、構成後のtarget resolverで解決 |
| Snapshot leaf / required open / delayed completion | 共通visibility target resolverを必ず通す。一致による直接openは維持し、ExtractedResultの順/一時表示で開く。本列を新規生成する経路だけmaterialize_bookを通す |
| Remote ZIP列挙とpage context、PDF、folder listing | 同pure ownerをcore workerから使用。Web側に並べ替え・pairingの第二実装を置かない |
| 戻り用保存一覧 / F12 fork / parked context | Bookだけ構成依存を保持し復帰adoption前に鮮度照合。結果列は構成依存/保存時更新なし (§10.1) |
| 検索・★・snapshot等の集約leaf open | 結果一覧の検索順は維持。open前にそのページのscopeの構成visibilityを解決する |

親containerの代表サムネ・pin、類似索引の元内容、メタ情報エクスポートは「本の可視ページ一覧」と別。
構成で非表示にしても既存pinや画像編集記録を破棄しない。代表画像まで変える要求は本項にないため維持する。

### 3.1 本・通常一覧・抽出一覧を生成ownerで確定する

`ProjectionRecipe::Book(scope, manifest) | OrdinaryList(surface, membership, order_policy)
| ExtractedResult(surface, membership, order_policy)` の排他的な列種別を使う。
構成の有無をleafごとに継ぎ接ぎするlayoutは作らない。現在anchorやsynthetic pathだけで分類しない。
Book資格は既存実効本分類 + 一つのscopeの直下実ページblock、動画/音声なし、抽出surfaceでないこと。
子container blockは既存分類に従い分離する。本扱いOFFは実効分類OFFの場合で、明示本置き場まで
自動設定OFFで解除しない。画像folderの現行資格を緩める変更はG4Fへ便乗させない。

| 列 | 表示契約 |
| --- | --- |
| 書庫実効階層/PDF/EPUB/適格画像本自身 | Book。可視ページは構成順/explicit units、詳細sort固定。構成なしは既存自動規則/§1.364 |
| Book内の一時facet/一覧内絞り込み | Bookのsubset。片側欠落は元slotのSingle、解除で元unitへ戻る |
| Ctrl+G/タグ/評価/スマートフォルダ/コレクション/★固定snapshot/サブフォルダ展開 | ExtractedResult (A2)。一冊分だけの検索結果でも入口の意味でこの契約。結果順で直接閲覧し、開くたびSingle |
| 抽出結果の詳細sort | ExtractedResultのまま、指定結果順で送り/一時pairing |
| 通常フォルダの詳細sort、スタック等の通常一覧、本扱いOFF、動画/音声混在 | OrdinaryList。従来の順・自動見開き・メディア閲覧を維持。保存構成をlayoutへ適用しない |
| 読書履歴/しおりの本を開く要求 | 対象のBookへ入り、その本の構成/本別設定を使う。ページ指定entryは§6の遅延照合 |

**A2の一時見開き**: 一覧全体に一つのAutomaticを置き、cover=false、shift=None、補助なし、
endpoint=Center、page_aloneなしを固定入力にする。元本・元ファイルのspread.db/構成は参照しない。
方向は全体既定の読む方向を閲覧sessionへsnapshotする (構成の方向は使わない)。既存landscape/non-pairable境界は
自動builderと同じで、別本の隣接画像は組になり得る。動画等を落とさず、別本の指定相方を呼び寄せない。
表紙・shift・末尾補助・端配置・強制単独の操作は構成の有無に関係なく全列で無効。
mode切替はsessionだけ、全体/本別/結果別DBへ書かない。新たにfullscreenを開く時はSingleへ戻す。
既存横長分割は閲覧投影として一時利用できるが記憶しない。F12で表示中の投影を複製する時はcapture-timeの
modeを共有できるが、その列へ次に新規openする時/固定一覧へ戻って再openする時にはSingleを初期化する。
`__search_results__` / `__rating_view__` / `__smart_folder__` 等の旧mode行を読み/書きしない。
既存行を一括削除・DB移行せず、抽出列の設定keyを生成しないことで過去値を適用しなくする。

**遅延非表示判定 (Q16)**: ExtractedResult/OrdinaryListのmembership、順、サムネイルは通常どおり。
列生成・サムネ取得では構成lookupをしない。「本の構成で非表示」のmarkerも作らない。
構成保存でこれらの列をreload/再prepare/nav再構築しない。対象をopenする時、送り/slideshow等で
次に提示する時だけ、そのidentityのscopeをworkerで解決しhiddenを照合する (§6)。
見開きなら実際に出す二つの候補を一つずつ照合する。全結果/全本の構成を先読みしない。
保存直後の表示中ページはそのままでもよく、以後の新規提示で非表示を迂回できないことを保証する。

画像folderの本扱いOFF/混在では保存構成を保持し、Bookへ戻れば再適用する (Q12/13)。
通常一覧の自動表示を使う例外であり、保存構成のbindingをHUD/キー/Remoteで編集する例外ではない。
その元folder scopeの既存構成は本設定変更受付時に照合し、§9.1のCompositionOwnedを維持する。
この照合は列の全leaf構成依存やhidden markerを要求しない。

結果から本へ入り直すことを強制しない。任意の「本を開く」は明示操作時に対象を解決し、
実際にBookへ移れる対象だけで案内する。本扱いOFF/混在元folderを開き直せば構成表示になるとは説明しない。
通常フォルダ詳細sortは抽出列扱いにせず、既存sortを保持する。sort lockはBook && accepted compositionのみ。
分類/layout/sort/操作資格は同じaccepted recipeから派生する。抽出列のshift資格のための
「構成本のleafが一枚でもあるか」の走査・scope依存は廃止する。

### 3.2 items行変更も投影ownerの更新である

PreparedBookの整数slotは `items_generation` と同時に採用される派生値であり、永続identityを代替しない。
初回installだけでなく、全行変更を同じ `update_projection` 境界に含める。

- **実削除**: `remove_items_batch` の既存old→new remap
  (`src/app.rs:43072`のshift、`:43081`のidentity表更新) をitems/metasと同じUI transactionで
  unit全occurrence・補助・anchor/selectionへ適用する。削除slotはNone、後方slotは同じ差分だけ移す。
  `[A,B],[C,D]`からBを削除した結果は `Single(A,元slot), Pair(C,D)`。AとCを組にしない。
  元manifest/identity解決表ではBをMissingへ更新し、構成DBのB参照は保持する。
  世代だけ進めて旧resolved_unitsを残すことは禁止。F5/workerを待たずunit/resume/exportは新投影を使う。
- **改名**: 成功経路 `src/ui_dialogs/rename_item.rs:172` は旧surfaceを解放し、migrationを起動し、
  親一致ならreloadを予約する (`:189`, `:202`)。専用PageRef移行をこの旧→新identity事象に含める。
  identity変更を反映できる列はpure remapを同時採用、再列挙が必要な列は既存reload完了まで旧投影を
  holdover専用とし、旧identityでのresume/export/編集を確定させない。単なるpage-edit rehydrate
  (`src/app.rs:45801`, `:45884`) だけでは構成identity/units更新の代わりにならない。
- **追加/外部変更/F5/policy・分類変更**: 新manifestとrecipeから共通ownerが再解決し、一括install。
  数値idxをidentityへ逆推測しない。Missingが同名identityで復帰したら指定unitへ戻る。
- **subset/詳細sort/復帰**: Bookは元投影のidentityに対するview投影、OrdinaryList/ExtractedResultは従来結果orderの投影。
  退避列を直接代入する経路も §10.1のadoption契約を通す。保存中にdraftの順をitemsへ差し込まない。

共通更新結果はitems/metas・identity/status表・layout・complete proof・依存revision・世代を一組として持つ。
公開consumerはその組のaccepted世代だけを見る。delete/rename/importは関連Bookの退避依存にも通知するが、結果列へ構成通知を配らず、
読み取りだけのfork/mount/closeは通知しない。resumeの互換rawは更新済元列、位置の正本はPageIdentity。

## 4. データモデル — 実ページと見せ方を分ける

### 4.1 BookScope / ページidentity

`BookScope { logical_root, level_prefix }` を使用する。rootは実フォルダ/PDF/EPUB/元書庫、
ZIP prefixは閲覧と同じcollapse後の実効prefix。通常ZIP、nested ZIP、ZIP内directory、直接RAR、
cache閲覧で同じscope resolverを使う。階層間のfallback継承は**しない**。
root構成を子階層へ適用すると、本の別ページをhidden/missing扱いするためである。

永続キーは既存 `spread_db::container_key` と同じroot+実効prefix合成を
`path_key::normalize_keep_drive` で正規化する案。`StoreKeyNormalization::KeepDrive`を登録する。
DriveStrippedなspread/resumeと異なり、利用者が作った構成を別ドライブの同名本へ誤適用しないことを優先。
PDF/EPUB/書庫ファイルのドライブレター変更・外部移動は既存content-identity restoreへ参加する。
**画像本フォルダは内容ID復元の対象外**。Folder内容hash/ledgerは新設しない。
G4Fで構成を同folderのmimageviewer.datへミラーし、backup設定ONでDB行がない時だけ自動復元する (§7.4)。
ページ参照はそのfolder内のファイル名。元のDB行は外部移動後も保持する。
backup OFF、sidecarをコピーしなかった場合は自動復元できず、明示メタ情報export/importで引き継ぐ。
アプリ内改名/移動は既存key migrationへ接続し、類似pathや一枚の画像内容IDから親構成を推測しない。

root aliasは独自に増やさず、
初回/再入場/しおり/Remote/export全てがcollapse済prefixを得る共通resolverを使う。
policy変更でcollapse結果が変わる場合、保存scopeはraw treeの実効階層を基準に同定し、
投影treeの新collapse先へ別構成を誤継承しない (受入テスト必須)。

構成内部の `CompositionPageRef` は既存 `book_bookmarks::PageIdentity` と同じ三種類:

| 種類 | 保存内容 | 解決 |
| --- | --- | --- |
| 画像本 | container相対ファイル名/path | 検証済みcontainer内の元manifestへ照合 |
| 書庫 | outer rootからの完全entry_name | entry正規化後、scope直下memberか照合。nestedのchainも含む |
| PDF/EPUB | 0-based物理page_num | 元PDF/元EPUBのページ番号。cache世代やitems ordinalではない |

`book_resume_db::PageIdentity` は絶対logical identityで、しおり型と別物。
一方をもう一方のserde形式として読まない。共通純粋adapterで
`(BookScope, CompositionPageRef) ↔ resume identity ↔ GridItem/RemoteAddress`を相互変換する。
保存値にcache ZIP/PDF、texture ID、items index、ページの表示番号は入れない。
EPUBは `BookScope.logical_root=元EPUB`, `level_prefix=空`, `PageRef=PdfPage(page_num)`。
編集キーも `normalize(元EPUB)::page_<番号>` (§2追加13) なので、別の変換世代/ページ対応表を作らない。
再変換後も同番号へ解決する。同じ入力/変換器のページ構造を前提とし、元内容・CSS・変換器等でページがずれた場合は
既存編集同様、同番号の別内容へ構成指定が掛かり得る。内容照合/自動reflow remapは行わない。
この制限をmanualへ明記し、必要なら解除→再作成を案内する。
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
             companion: Vacant|Blank|FrontCoverSupplement(PageRef) }
  ],
  hidden_pages: set<PageRef>
}
```

`first/second`は読み順。Leading/Trailingをdirectionで画面Left/Rightへ変換する。
したがってRTLを変えても実ページ順は反転せず、slot側だけ鏡映する。
`Center`のcompanionはVacantのみ。Singleの片側とVacantは片側配置だけ、Blankは白い面を描く。
`Blank`はSingleの相方という値だけで、独立ID/source address/PageIdentity/選択行を持たない。

Pair/Singleを正本とし、「フラットpage/blank列 + pair上書き + singleton上書き」の三つを別々に保存しない。
flat列はunitを展開して得る派生値。実ページの通常occurrenceは構成全体でちょうど一つ。
hidden_pagesはその登録済PageRefのvisibility指定で、**unitsから参照を取り除かない**。
非表示/表示に戻すだけでは元Pair/slotを保持し、resolverがその側を欠落投影する。
これで非表示解除用の旧位置/旧Pair退避stateを別に増やさない。白紙と実ページを組んだunitはSingle、二実ページだけPair。
**相方の実ページ参照を持たない白紙は保存できない (Q4代案採択)**。
blank-only unit/連続blank/二つ目のcompanionを型/保存検証で拒否し、draft操作も適用前に検証する。
連続判定はdraftの読み順slot展開でも検証する。例えば `Single(A,Leading,Blank)` の直後へ
`Single(B,Trailing,Blank)` を置くと展開が `[A,白紙,白紙,B]` になるため、二つとも実参照があっても拒否する。
方向による画面鏡映では判定を変えず、移動/端配置変更も同じvalidatorを通し、拒否で元draftを保持する。
実ページを動かす時は白紙をそのSingleに付随させる。白紙だけを移動・選択する操作はない。
Pairへ変更する時は白紙を外すことを明示した同じ操作にし、余った白紙を別unitへ退避しない。
相方PageRefがHidden/Missingならそのunitは閲覧に出ないが、実参照と白紙指定は保持する。
実参照のない白紙とは区別し、FS欠落を理由に利用者指定を消さない。

末尾補助の表紙だけは `FrontCoverSupplement` の再利用を許す。普通のページ複製機能は範囲外。

保存検証では、種類一致、安全なrelative/entry、scope内member、通常occurrence重複なし、hiddenが登録済refの部分集合、
companion/placement整合、補助が指定coverを参照すること、相方なし白紙の不在、件数/文字列/JSON bytes上限を確認する。
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
ここで取り込むのは現在の見え方のsnapshotであり、全体/既定設定や既存spread.dbへの書戻しではない。
方向、表紙、shift適用後の組、末尾補助、端配置、強制単独の結果を一つの初期recordへ保存する。
作成成功/取消/失敗のいずれでも全体/既定設定を変更しない。作成後はそれらをresolver入力から外し、
全体設定変更や別の本のHUD操作がこの本の方向/表紙/unitsを変更しない。
構成なしSingle/Splitから作るときのspread初期方向は既存本別/全体の値を解決してプレビューし、
Single/Splitの閲覧モード自体をペア指定と混同しない。

並べ替えは実ページまたはunitを編集draft内で移動し、**操作した連続範囲だけ**明示unitへ整形する。
Pairからページを抜いたら相方は元の側のSingle、貼り付け先の「この2枚を組にする」でPairへ変更する。
「白紙を挿入」は選択実ページをSingleにし、選んだ側の空きをBlankにするdraft操作。
既に白紙相方があるページへの二枚目、白紙だけを残す操作は
「白紙には組になる実ページが必要です。白紙だけの組や連続した白紙は作れません」と通知してdraftを変更しない。
非表示/実削除によるMissingでは参照自体が残るため白紙指定も残す。
本全体の残りを暗黙にparity再計算しない。「自動の並びから作り直す」は明示操作として確認を挟む。
構成ありで作り直す場合も、draftの方向/表紙とeditor内で明示選択した条件だけを入力にする。
全体/既定設定を再適用する入口にはしない。変更対象の組をプレビューし、確認したdraft保存でだけ置換する。
draftでの戻す/やり直しは在メモリの編集だけで、永続rollback logを作らない。

方向/表紙/末尾補助/端配置/単独表示も、このdraftを編集してプレビュー後に一回保存する。
方向はLeading/Trailingの画面slotを鏡映し、実ページ順・Pair membershipは保つ。
表紙指定は選択PageRefのSingle化と補助参照の整合を同じdraftで行う (§4.2の検証)。
補助表紙は選択Singleのcompanion、端配置は選択Singleのplacement、単独化は選択Pairの分割として扱う。
Pairを解いた相方は元slotのSingleに残し、未操作のPairを自動規則で再構成しない。
中央配置へ戻す/表紙指定を外す場合も、白紙/補助への影響をプレビューして明示確定する。
「一ページずらし」のsession actionはeditorにも持ち込まず、組の変更をdraftの明示unit操作で行う。
閲覧中の設定toggleを構成へ翻訳する別command service、FollowGlobalフラグ、旧Pair復元logは作らない。

§1.239/§1.240/末尾補助のフラグは作成時に実際のunitへ変換され、その後の自動規則ではない。
補助表紙は特定unitに付くpresentation roleとして保存し、新ページ追加後もそのunitに残す (Q3決定)。
新ページは末尾中央Single。途中挿入で並べ直すのが難しければ、利用者が構成を解除して再作成できる。
解除の影響を確認し、再作成はその時点の自動規則/実効shiftを初期値へ取る。自動解除/自動再作成はしない。
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

単ページ閲覧はvisible実ページのflat列、blank/補助なし。分割閲覧はそのflat列を既存
`page_split::PresentationStep`へ渡す。Split halfはPageIdentityと別のpresentation identityを維持する。
Remote `portraitSinglePage/force_single_page`も同じsingle投影。端末の向きからDBを書き換えない。
連結読みは同じexplicit spread unitsを使い、anchor・seekとgeometryの対応を保持する。
現在のpage_edit pivotがSingleへ移っても構成recordは変更しない。

Book内の一時facet/検索欄は構成順で片側欠落を上記投影にする。
ExtractedResultは§3.1の固定Automatic、OrdinaryListは従来Automatic。複数本のComposedを継ぎ接ぎしない。
抽出列の組は元結果のnav隣接で決め、unit提示時にだけ候補のhiddenを確認する。
片側Hiddenなら残る側を元slotのSingleへ提示し、別unitから相方を呼び寄せない。
両側Hiddenならそのunitを飛ばし、次unitの候補だけ順次確認する。単ページでは候補ページだけ確認する。
提示時の欠落はDBへ書かず、全結果のnav eligible列を先に作り直す機械も追加しない。
全候補を通過する場合も取消可能な既存ナビworkerの候補走査で終端へ進む。
明示targetのHiddenはtoast拒否、送りではskip後に「本の構成で非表示のページを飛ばしました」と通知。
全候補Hiddenならfullscreenを閉じ一覧を表示する。既存非pairable境界は維持する。

### 5.2 キャッシュのキーと失効

PreparedBookは `(scope, source_manifest_revision, policy, composition_revision)` を持つimmutable snapshot。
ここでcomposition_revisionはDB rowの数値だけでなく、serviceがscopeの採用値 (構成なしを含む) に付ける
`(service_epoch, publication_sequence)` を含むtoken。変更したscopeだけ新tokenを発行する。
解除→再作成で同じrow revisionへ戻っても一致しない。service再初期化ではepochを替え、
退避値を旧tokenで復帰させない。新たな永続tombstone/dirty storeは作らない。
構成変更は既存installで必ずitems generationを進め、index/memoを同時に作り直す。
行削除等のin-place更新でも §3.2のremap後に同じ世代を採用する。
OrdinaryList/ExtractedResultはmembership/order/source/policyの既存revisionだけを保持し、構成依存を持たない。
構成保存による一覧/nav/unit失効は発行しない。遅延admissionの返答は対象identity/要求ID/context/items世代/そのscopeの
publication tokenに相関させ、提示確定前に構成が変わった返答は採用しない。次の提示要求 (スクロールによる新入場を含む) で新値を照合する。
全列hidden cacheへ溜めず、構成なし本も含め新規提示の候補だけ最新snapshotを確認する。
結果のbase unit cacheには元の隣接組だけを保存し、提示時のHidden欠落をそこへ焼き込まない。
allowed候補/相方と照合tokenを含む要求所有のpresentationを描画/holdoverへ渡し、geometry/rendition keyにも含める。
連結では§6.2のviewport demandが要求ownerとなり、同一画面内の継続提示だけ許可を保持する。
過去の許可やSingle化された描画cacheを次のopen/nav/viewport再入場/mode投影の許可として再利用しない。
`ViewerNavigationCaches` の世代/flowにはこのinstallが含まれるので、新たな「構成dirty」boolは不要。
unit tokenはLayoutSourceのtagとcomposition revisionを明示追加する。Composed branchにshiftは入らない。
nav列、items世代、Single/Spread/Splitの閲覧投影、direction、filtered projection identityを含める。
Automatic branchのlandscape epoch/page_aloneはそのまま必要。
geometry cacheはさらに既存寸法/回転/trim/scale revisionを使う。手動Pairのtopologyをlandscape epochで変えない。
G0/E接続: fit/geometry入力は既存 `FullscreenFitInput` のA/O・指定値・共通capとlimits、media rect/pppを含む。
Gの新unit token/提示許可とこれらのdescriptorを対にし、同じidxや完成textureだけで旧geometryを再利用しない。
capture/replayは捕捉descriptor、live連結は保持source + 現在trim/rotationから解くEの規則に従う。

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
- Remote container resume、bookmark jump、直接full-page/AI/read/write address。結果のthumbnailは通常取得し非表示照合しない。

直接file openも親scopeの既存構成をworkerで照合してからpageを受理する。
「現在のitemsにない」だけの判定では、集約leafを一枚だけ開く経路やRemote direct addressを止められない。
既存open admission/target resolverに相関済み結果を渡す。
拒否時はopen/switchを確定しない。旧本・旧fullscreen/履歴・return routeは保持し、
先頭や隣ページへsilent fallbackしない。containerを一覧として開く要求自体は許可し、
非表示resumeのauto-fullscreenだけ拒否する。全非表示本は空一覧と「本の構成を編集」の導線を示す。

遅延照合は既存open/nav要求owner、連結では§6.2の表示ownerが候補要求をworkerへ渡し、対象PageIdentityからlogical scopeを確定して行う。
UIでSQLiteを読む、現在mountの本から推測する、列生成で全本を照合することは禁止。
クリック/しおり/評価/履歴の明示targetはHiddenならtoast拒否し、現在surface/return routeを保つ。
送り/slideshowの候補と見開き相方にも同じ受付を通し、Hidden画像を新たに提示しない。
連結のスクロールは明示navを経由しないため、可視候補からの要求とpaint境界を§6.2で具体化する。
サムネ/通常prefetchは従来どおりで、画像取得とfullscreen提示の許可を同一視しない。
飛ばしたページを「読んだ」と記録せず、保存通知で既存表示を回収するlive refreshはしない。

保存で現在anchorがHiddenになる**全更新対象Book context** (editor元窓、同一本の兄弟、parked/退避Bookの
後の復帰を含む) は同じ着地規則を使う (Q5決定)。結果列はQ16で保存時更新の対象外。
結果列の現在画像は即移動せず、後のopen/nav/退避fullscreen復帰の新規提示時だけ照合する。
退避結果のanchorがHiddenなら次→前→先頭→一覧を候補ごと遅延評価する。
各contextが変更直前に採用していた読書flat列を基準に、旧anchorより後の最初の新可視identity、
なければ前方を逆順に探した最初の新可視identityへ移る。同時に並べ替えても基準は変更前の順。
旧列の候補が全て消えたが新tail等があるなら新列先頭、可視navが全くなければfullscreenを閉じて空一覧。
Bookは本の旧構成順、遅延復帰する結果列は旧結果/詳細sort順。動画/音声anchorはhidden変更の対象外。
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
  現在元窓がOrdinaryList/ExtractedResultでも、共有位置の候補を別本・動画・音声へ跨がせない。
  個別窓のfacet/結果順での着地とは分け、Book投影のpolicy採用済み可視実ページだけを使う。
  全非表示/seed欠落で安全な位置が決まらなければKeep。次回のhidden resume拒否は従来の本設計どおり。
- saveの準備workerが新投影とこの位置を純粋に解決し、composition commit成功replyの
  **要求完了owner一か所**が既存resume writerへ最大一回enqueueする。raw・identity・meterは同じ新Book snapshotから得る。
  enqueueは成功replyの処理内で各窓のreload/adoptionを開始する前に行い、後の通常操作のwriteより遅れて発行しない。
  commit失敗/cancelは書かない。各context reload完了を待ってownerを選ばない。
  同値なら既存dedupで省略し、独自の永続exactly-once台帳やretryを作らない。
- 既存open/navの `record_book_resume` (`src/app.rs:56814`, `:65343`、`src/ui_fullscreen.rs:21359`, `:39152`) は
  保存更新によるreload/adoption/reanchorを「新しい読書操作」として通さない。
  呼出し起点をtypedなProjectionRefreshとして既存install/open効果へ渡し、anchor/メモリのmeterを更新しても
  resume/history書込効果を発行しない。parked復帰・退避一覧の鮮度更新も同じ起点。
  per-contextのdedup値が違うことを、共有writerの優先順位代わりにしない。

意図した通常のページ送り等が再び許可された後は、従来の読書操作が位置を更新する。
更新中の操作はmodal/既存loadingで止め、旧reload返答を後の読書writeとして配送しない。
Remote取得がSavingを閉じた場合も保存済replyの同じintentだけを回収し、ローカル窓の終端から再記録しない。
閲覧mode切替は構成保存を発行しない。背景のprojection更新でもreloadからresumeを記録しない。
metadata import等に伴う構成投影の背景更新も、新しい読書操作ではなくKeepを使い、表示窓/退避列の鮮度だけを更新する。
現行metadata transferはbook resume/履歴を転送しない。構成importの背景更新も新規resumeを書かない。
Q5の決定は「次回の再開位置は保存要求の元位置に基づく一つ」であり、兄弟窓の移動先を奪い合わない。

G0照合: 通常open `src/app.rs:65343`、continuous seek `src/ui_fullscreen.rs:21359`、
continuous reanchor `:39152` は `record_book_resume` を直接呼ぶ。HistoryTriggerだけを弱めてもresumeは止まらない。
Book reload/復帰は既存のtyped navigation/open purposeへ**投影adoptionの原因**を渡し、その要求終端まで
通常resume/history発行を行わない。新しい一時suppress boolや窓の優先順位を足さず、保存intentと別のcauseであることを
受付からreanchorまで保持する。adoption後の寸法/fit後着による自動reanchorもこの投影causeを引き継ぎ、
最初のpaintやreload完了だけで通常読書writeへ切り替えない。次の通常ユーザーナビ/再開した自動送り要求を
受理した時に既存閲覧ownerのpurposeを通常読書へ更新し、以後は従来どおり記録する。
Eのfit参照再選択や初回layoutの自動reanchorも、adoptionから発生する間は同じcauseへ従う。

### 6.2 連結読みを含む新規提示の許可owner

連結読みは維持し、nav sequenceを強制作成したり、anchor更新まで照合を遅らせたりしない。
既存 `ContinuousReadingUnitSpec.presentation` (`src/ui_fullscreen.rs:8224`) / 可視位置列 / 描画ownerを使う。
そのcontextのcontinuous表示要求へ **viewport内のpresentation occurrenceの許可** を載せる。
要求は `ViewerContextBundle` の既存continuous runtimeと一緒にmount/deposit/fork/closeされる
(現在のscroll/layout所有: `src/app/viewer_context_registry.rs:991`, `:1054`, swap `:2646`, `:2693`)。
App共通bool、anchor専用許可、texture cacheに付ける許可の別ownerは作らない。

責務と確定順:

1. `draw_fs_continuous_reading` が `continuous_reading_layout` の可視候補を受け取った時、
   **texture選択より前**に、新入場のsource identity/occurrence (unit anchor・role・sliceを含む) を
   表示要求ownerへ渡す。scroll/wheel/drag/gamepad/resize/scale/寸法後着/slideshowで隣ページが
   見え始める時も同じ経路。準備band/keep-set/prefetchへ入っただけでは提示許可を作らない。
2. 許可は要求内のtyped outcome `Checking | Allowed(scope_token) | Hidden(scope_token)` として持つ。
   workerはその画面で新たに提示する候補だけ照合し、同じsourceの補助/sliceは一要求内で照合を共有できる。
   結果採用はcontext/要求ID/items世代/occurrenceとscope tokenで照合する (§5.2)。
   未許可候補の領域は既存loading相当の背景/待機表示だけ。UI同期DB/file I/Oや全結果列の照合はしない。
3. 新しいunitの提示確定は、そのunitの全presentation候補の照合完了を待つ。
   sourceごとの判定を途中で一枚だけ採用せず、Allowed Pair/片側HiddenのSingle/全Hiddenのskipをatomicに決める。
   Allowedだけを**提示用pagesとtexture選択入力**へ渡す。texture選択ループ (`src/ui_fullscreen.rs:39285`)
   と `draw_fs_spread_page` 呼出し (`:39395`) の両方で許可済みprojectionを使う。
   Checking/Hiddenにはcached processed/raw/transition/thumbnail/pass-throughのどの画像も渡さない。
   paintだけでなく `fullscreen_page_layout` 登録、hit test/編集target、navigator画像、presentation trace、
   読了/位置記録も同じ許可済み投影に従う。キャッシュに画像があることを許可とみなさない。
4. Hiddenはその表示要求内でskipする。片側だけなら元slotのSingle、両側なら次unit候補へ進む。
   連結の可視投影/offsetも同じ許可済みunitで解決し、Hiddenだけの空段を残さない。
   新候補が入るなら既存可視ページ上限に収まる分だけ次のworker要求へ渡す。anchorを先に変更せず、
   描けたNavigationから既存reanchorを行う。全Hiddenで候補を使い切った時だけ一覧へ戻る。
   base items/nav順は変えず、他unitから相方を借りない。明示openのHiddenはtoast拒否、連結の隣候補はskip。
5. 画面に残る同じoccurrenceは現在の提示episodeの許可を保持して継続描画できる。
   scroll値・reanchor・viewport寸法の変更だけでは退出としない。探索cursorを作り直す場合も、
   既存の可視/Checking occurrenceを保持し、測定後の可視要求との差分で退出をretireし新入場だけを照合する。
   同episodeでskipした候補も要求内だけに持ち、毎frame同じHiddenを問い直すloopを作らない。
   画面から退出/要求retire/closeで提示許可を破棄し、再入場は新要求。Hidden除外は当該可視要求内だけで、
   全列skip cacheや永続非表示bitmapへ昇格させない。
   構成保存通知で結果列/表示要求を一括refreshしない (Q16)。既に表示中の画像は残り得るが、
   未提示候補・再入場は最新値を照合する。Checking後着は提示直前にtokenが古ければ採用しない。

Single→Spread、Split切替、補助表紙の新出現も同じ **presentation demandの差分** で受け付ける。
既に表示中の実ページと新しい相方を区別し、新しい相方はcached textureがあっても無許可で出さない。
新しい二ページ投影はCheckingを含む間はatomicに未確定とし、旧許可済み表示/背景を保つ。
確定後はAllowed PairまたはHidden片側を除くSingleを一回で採用する。page-turn sequence/holdoverの
役割別presentation、連結のviewport要求が同じ許可resolverを使い、mode handlerに別DB照合を増やさない。
Bookは採用済み構成snapshotの可視列を入力にでき、OrdinaryList/ExtractedResultは候補ごとにlogical scopeを解決する。
Eの指定サイズ参照を新たに選ぶ時も許可済みunitだけを渡す (§13 G0照合記録)。未許可/Hiddenの新しい相方を
fit基準やそのunit自身のgeometryへ確定させてからpaintで隠す実装にはしない。
一度許可して保持した画面外の参照sourceはmeasurement用に継続できるが、それは再入場の提示許可を兼ねない。
read-only mount/forkは画像と同じ捕捉済み表示を継続できるが、fork先の新入場/新投影はそのcontextの新要求となる。

## 7. 新しい本別ストアと既存ライフサイクル

### 7.1 保存

`<data_dir>/book_composition.db`、一つの `book_compositions(book_key PRIMARY KEY,
revision INTEGER, schema_version INTEGER, composition_json TEXT)` を提案する。
JSON内は相対PageRef/物理ページ番号のみ。全record一transactionのreplace/deleteで、units別テーブルを増やさない。
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
破損/未知schemaならDBを作り直さず、通知・書込停止・構成本の閲覧拒否とする (Q7決定)。
構成は利用者作成データなので、再生成可能なサムネcacheと同じ扱いにはできない。
稀なcommit後reload失敗は保存済recordを保持、通知して既存再読込または再起動を案内する。
表示を新構成で完成したと偽らず、失効した旧indexで編集/移動を続けない。
これ以上の回復が必要というレビュー指摘は、先に利用者へ割り切り案と費用を相談する。
明示「今回は構成を使わず開く」を用意する (Q7決定)。
選択したopen要求のcontext/scopeに限るAutomatic投影で、DB row/他contextのaccepted構成を変更しない。
shiftは許可せず、構成編集も書込停止中は受けない。次のopenへ選択を記憶せず、独自の復旧状態を永続化しない。

### 7.2 登録先チェックリスト

| 接続先 | 必要な処理 |
| --- | --- |
| `rename_key_migration::STORES` | KeepDrive/uniqueな本行を登録し、exact/`/`/`::`prefix rename/copy/probe/purgeの対象へ。DB未作成は正常不在 |
| 画像個別改名/移動 | しおりの`migrate_paths_at`同様、該当containerのunits/hidden/cover内のrelative PageRefをまとめてtransaction更新。page移動がscope外ならMissingを保持。root改名だけではJSON内変更不要 |
| archive内entry変更 | 本体から行う対応操作があれば同じPageRef移行を呼ぶ。外部rewriteはMissing保持で手動再対応。ページ番号から別entryを推測しない |
| 実ファイル削除purge | 本/rootの削除成功は構成行/prefixをpurge。ページ一枚削除では親構成をpurgeせず参照をMissingとして保持。非表示操作はFS/purgeを一切呼ばない |
| コンテナcopy / 内容ID復元 | generic `copy_stores_at`の本行とvirtual-prefix mappingへ参加 (`content_identity/restore.rs:414`)。relative/page-number payloadなのでcache mapping不要。destination既存構成を上書きしない既存conflict規則を使用。Folderには内容ID復元を約束しない (§4.1) |
| restorable probe / metadata cleanup | 構成も復元可能なuser dataとして数える。cleanupラベル、DB件数/clear、設定完全reset、data-dir移動・保存先説明へ登録。resetは既存明示確認の範囲で行う |
| 明示metadata export/import | §7.3の専用composition familyを追加。既存PortableContainerStateのorigin/挿入keyと分離。root/prefixはbundle相対、page refsは本相対。is_empty/has_view_state/summary/import changed flagsも更新 |
| metadata import refresh | owner/context/policyを捕捉し、関係する同一本scopeのBook projectionを既存reloadで更新。結果列は更新せず、別本のindex/cacheを捨てない |
| 自動sidecar | G4Fの画像だけのfolder構成をroot字段へ追加。全snapshotのwriter/予約/reuse、独立構成familyの内容token/消費receipt、backup設定/不足行復元は§7.4。書庫/PDF/EPUB構成は対象外 |
| F12 | registryのnew/default/destructure/swap/mount/fork/park/drop、親一覧/固定snapshotへprojectionを運ぶ。cache ownerはcontextごとに独立 |
| Remote | core engineにread snapshot/lookup登録、scope resolver・list/page context・visibility admission・payload登録。IPC版とWeb asset tokenを同時更新 |

構成保存/解除、rename/import/restoreが実際に変えたscopeだけに変更通知を出す。
読み取り専用open/closeはmutation-style invalidationを出さない。
content identityは元メディアの内容であり、構成revisionを内容hashへ混ぜない。
**構成保存成功時**には、要求時に捕捉したlogical rootへ `ContentIdentityTrigger::Edit` を送る。
現行 `record_current_container_content_identity` (`src/app.rs:25962`) の考え方を使うが、
reply時のmount/current_folderを参照しない。PDF/EPUB/ZIP/対応元書庫は新規構成だけの保存でも
restorable ledgerを作る。nested保存もouter logical rootを観測する。Folderは非対応として送信対象外。
editor保存/importによる構成作成は同じ成功通知へ接続する。Remoteに構成write入口は作らない。
STORES/copy/probe登録のみで観測済扱いにしない。
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
構成専用rootは常に元フォルダ/PDF/EPUB/元書庫のbundle相対path。cache origin選択肢はない。
export照合はKeepDrive、importは搬入先logical rootをKeepDriveで正規化しscopeをjoinする。
通常のspread/trim familyは今のDriveStripped source/cache選択をそのまま使う。
scope prefix/relative refsを安全に検証し、受入先treeの実効scope resolverと整合させる。
cache rootを論理rootへ再分類して既存origin競合を避けるという改変はしない。

従って「元RAR側のroot/nested構成 + cache ZIP側のroot/nested spread/trim」は通常の正当なpayload。
構成以外の既存family内source/cache競合は従来どおり検出する。構成がその競合の理由になってはならない。
export選択でfolder entryが省かれる方式でも、構成対象のlogical folder entryをfamilyの搬送ownerとして
明示列挙し、子Image entryの構成として扱わない。画像folder bundleの受入試験でこれを証明する。

不在の意味はwireで三値を区別する (Q15決定):

- `composition_family`字段/sectionなし: 旧版/構成を扱わないpayload。既存構成を保持。
- 新版でsectionを含み `{ scopes: [] }`: covered logical familyの構成なし。受入先構成を解除。
- 非空scopes: covered scopeを丸ごとreplaceし、coverage内の記載されないscopeは解除。
  coverageはrootのみ/明示prefix集合/family全体を明示し、対象外階層を巻き添えdeleteしない。

新exportは構成のない対象本もfamily/coverageを記録する。folder entryも搬送ownerとして列挙する。
物理項目の照合でMissing/KindMismatch/Changed等のskipになった本は、その既存構成を保持。
旧形式不在を新版空へ変換せず、構成sectionなしを「構成なし」と推測しない。
任意のページPair同士をmergeせず、本行は既存の編集・spread等と同じ置換規則へ揃える。

既存ImportConfirm一回に「本の構成: 追加○冊・置換○冊・解除○冊」と
「構成なしと記録された本では、取り込み先の構成を解除します」を加え、同じインポートボタンで一括承認。
本ごとのprompt、カテゴリ選択checkbox、別の構成置換承認画面は追加しない。
多数本・全構成なしのpayloadも一回。previewはworkerでread-only照合し、適用時は通常のtarget再検証を行う。
preview以後の変更があれば最終summaryの実適用数で説明し、per-bookの再確認へ分岐しない。
輸入元revisionは採用せず、受入先writerが実変更時にlocal revision/tokenを進める (§5.2)。
構成と既存spread/trimのtarget keyは別々に計算し、既存import batch/SAVEPOINTへ参加。
全件一transactionにはせず、cancelで適用済batchを保持する既存契約を維持する。
resume/履歴は転送しない。完了通知はBookに限定。画像folderは§7.4のbackup/旧sidecar消費とも接続する。

### 7.4 画像だけの本フォルダのサイドカー (G4F限定)

Q10決定: `mimageviewer.dat` のrootへ任意の `folder_composition` を追加する。
SidecarJsonのitems内の偽ページではない。構成にはfolder内ファイル名のPageRefを持ち、absolute root/cache pathは含めない。
既存SidecarFileのcache/dirty owner/worker flushを共有し、編集・タグのitemsを消したり別writerで全fileを書き換えたりしない。
`replace_edit_bundle`等の既存ページ更新もroot構成を必ず引き継ぐ。
書庫/PDF/EPUB構成はこの字段へ入れず、content-identity restore + 明示bundleだけで運ぶ。

writeと自動restoreは `sidecar_backup_enabled` (既定ON) に従い、構成専用toggleを作らない。
OFFならDBだけ保存し、構成字段のwrite/restoreをしない。タグbackupだけONでも構成は対象外。
既存タグbackupの書込は維持し、構成字段を勝手に新しいDB値で上書きしない。
ONで保存/変更/解除/importした場合は、captured folderのroot構成を新record/不在へミラーする。
アプリ内の個別改名/PageRef更新も同じmirrorを更新し、root改名/移動はfolder内のsidecarを既存FS経路で運ぶ。
DBが正本。folder開場時の既存restore workerへ構成の検証/不足行挿入を追加し、**確認質問なし**で自動復元する。
DBの既存行 (読めない/未知schemaも含む) は上書きせず、通知・書込停止を優先する。
安全なfile名/型/上限/blank規則はDB保存と同じ検証。各参照の実体欠落はMissingとして保存できる。
復元成功後のBook materializationを継続し、復元前のraw列で一瞬fullscreenを開かない。
別folderへの外部移動/ドライブ変更でDB行がない場所にsidecarも運ばれれば復元でき、旧場所のDB行は残す。
本扱いOFFでも画像だけの実folderの不足行は復元できるがlayoutはOrdinaryListのまま。
混在/製本/抽出列はfolder構成restoreの対象外。結果からのページopenではDB行だけ遅延照合し、各leafの
sidecarを全て読み直す裏口を作らない。必要な自動復元は明示folder open時に行う。

現行 `SidecarJson.version=1` (`src/sidecar.rs:54`, `:66`) はreleased。
G4Fでversion 2へ上げ、readerはv1を「構成字段なし」で保持して読み、writerは既存items/タグを含めv2で保存。
未来版/破損sidecarは現行write-disabledの原本保護を維持。新DBと違い既存sidecarを捨てて作り直さない。
旧アプリがv2を読めない点はmanualへ記載する。G4Fを切るならsidecar変更そのものを入れない。

#### 7.4.1 rootを含む単一snapshot・writer・再利用証明

現行items Arcだけの契約を、immutable `SidecarSnapshot { items, folder_composition }` 一つへ拡張する。
SidecarFileはそのArcを所有し、items/rootのどちらの変更もcopy-on-writeでsnapshot identityを替える。
既存items mapのArcはsnapshot内で共有でき、rootだけの変更で大量のページ編集mapを複製しない。
root専用dirty bool/別flush queueは作らない。write requestはsnapshot全体を渡し、
**itemsが空かつroot構成もない時だけRemove**。構成だけのsidecarを保存でき、最後の個別編集を消してもrootは残る。

`PendingWriter` のsequence/kindとsnapshotは不可分。現行 `revalidate_import_source` (`src/sidecar.rs:442`)
のWrite照合を全snapshotのArc同一性、Remove照合を全snapshotの空判定へ置換する。
PendingWriterのsequenceは現行どおりprocess内だけで、diskのsync markerへ保存しない。
`LoadedSidecarImport` / strict prepareの検証対象・requested family・retained probeのreuse keyにもroot構成を加える。
itemsだけ同じ、またはedit markerだけ同期済みを根拠にrootのprobe/importを省略しない。
Disk reuseは現行pending/failed writer優先、metadata-before/after + SHA-256再検証を保持する。

`SidecarFlushReservation` とworker batchはfolder/全snapshot Arc/disabled/requestedを捕捉する。
flush完了がdirtyを消す/clean ownerをevictする条件は、現cache ownerとこの全snapshotの完全一致。
itemsが同じでもrootが変わったら旧flushは新rootのdirtyを消費しない。
退出/終了/明示importのflush fenceも同じ予約を使い、他contextの新snapshotを取り出して破棄しない。
worker failureは現行の通知/dirty owner保持の契約を使い、rootだけのretry/rollbackを追加しない。

#### 7.4.2 自動復元の内容tokenと旧構成の消費

構成は既存sidecar import engineの **独立したfamily** とする (edit/tag familyの同期判定を流用しない)。
これは二つ目のimport ownerではない。既存prepare/probe/revalidate/workerとmaintenance/quiesceを共有し、
新composition storeの同じwriterで不足行挿入と同期receiptを一transactionにする。
commit直前のsource再検証を維持し、そのtransaction内で現在の構成行/receiptを読み直して判定する。
古いprepare結果で、途中の明示解除/新構成保存が作ったreceiptやrecordを上書きしない。
既存edit/tag DBのmarker/schema/不足行補充を変更せず、構成だけ未適用なら構成familyをImportRequiredにする。

構成familyの同期receiptはfolder keyに対し `{ disk_sync_marker, consumed_root_digest }` を持つ。
`disk_sync_marker` は**実際に厳密loadしたSidecarDiskToken.sync_marker()**。mtime秒値を記録しない。
`consumed_root_digest` はそのsnapshotの検証済みroot構成 (不在を含む) の正規化内容hash。
タグ/itemsやfileのmtimeを含めず、同じ古いrootが残ったまま他familyだけ更新されたことを証明するために使う。
完全一致tokenでAlreadySynchronizedにする既存probe契約は維持し、token不一致ならstrict probeで再検証する。
その後、構成familyだけについてroot digestが既存receiptと一致すれば、そのrootは消費済みとして
**不足DB行へ再挿入せず、今回の実tokenへreceiptを進める**。別のrootなら通常の不足行補充候補になる。
全folderを永久に復元禁止にするtombstone、消費token履歴、タグworkerからのDB更新callbackは作らない。
root witness一つが必要なのは、全fileのtokenだけではタグ書込後の旧構成を区別できないため。

明示解除/新形式metadata importの空反映では、backup設定OFFでも、captured folderの既存sidecar rootを
workerで厳密snapshot化し、**DBの構成行削除とその消費receiptを同じcomposition transaction** へ入れる。
既存pending writerは全snapshot/fenceの優先規則で確定させ、旧disk tokenと別のpending rootを組み合わせない。
Missingなら消費するbackupなし。読めるsourceの消費証明を確定できない場合はDB commit前の保存失敗として
Q7の通知/旧構成保持に従う。未知/破損sourceは原本保護し自動復元もしない。
この準備とDB writeはworkerで行い、UI同期readを足さない。DB成功後のsidecar mirrorの失敗とは区別する (§7.4.3)。
ONなら共有writerへroot不在の新snapshotも渡すが、旧fileが残ってもreceiptで同じ場所の復活を防ぐ。
明示importのmtime秒値記録には依存せず、import対象folderの空反映もこの構成familyの処理へ接続する。
旧形式/coverage外/skipは構成/receiptを解除せず、Q15どおり保持する。

**OFF→解除→タグbackup更新→ON→再入場**:
解除時に旧backup rootを実tokenと共に消費。OFF中のタグwriteはitemsだけを更新しrootを保存したままなので、
ON後のfile tokenは変わる。strict probeは新bytesを検証し、同じroot digestを根拠に構成は再挿入せず
新tokenを消費する。edit/tagは各自のmarkerどおり処理でき、構成の消費で新しいタグ復元を抑えない。
同じbyte length/mtimeでrootが変更された場合もSHA-256/root検証で見分け、旧reuse proofを採用しない。
新しい場所ではfolder keyが異なりreceiptがないため、運ばれたsidecarから通常の不足行補充を行う。

receiptは転送する利用者構成ではなく、**その場所のbackupの消費記録**。
metadata export/import・content-ID copyではreceiptを運ばず、import destinationのbackupから新たに作る。
アプリ内folder renameでは移動先の実sidecarとの照合後だけkeyを更新し、purge/resetでは同じfolder範囲を除去する。
receiptはApp共通のstore (DB) に保持し、F12 bundle (viewer contextの実行時状態) へは運ばない。F12 bundleが運ぶのは構成の投影と提示許可だけ。構成なしでもreceiptの存在をDB構成ありとは扱わない。
自動restoreによる新record採用、またはDB既存行を保持したrootの消費も、検証したsourceと同じtransactionで記録する。
PendingWriterでもroot witness一致なら再挿入しない。適用時にsequenceをdisk markerへ変換せず、
既存disk receiptを保ち、diskに載った全snapshotを次のstrict probeで検証して実tokenを記録する。

#### 7.4.3 sidecarだけの保存失敗 (利用者採択)

DB保存/解除成功後にbackup writeだけ失敗した時は、**DB結果を維持**して
「フォルダのバックアップを更新できませんでした」を通知する。
古いbackupが残る可能性、移動前の明示保存またはメタ情報書き出しを案内する。
同じ場所の解除済みrootは§7.4.2のreceiptで復活を防ぐが、旧backupを別の場所へ運べば旧構成が復元され得る。
自動retry/DBとfileの同時rollback/終了後draft復旧を追加しない。既存writerのfailed判定は保持し、
明示再操作または再起動の案内に留める。DB失敗のQ7、保存後reload失敗、backupだけの失敗を混同しない。

## 8. エディターUI — モーダルを採用する

本全体のorder/indexを確定する短い操作であり、数十分のダウンロード管理と異なるため、
準備→編集→保存→reloadをモーダルにする。閲覧プレビューはdraftの専用領域で行い、
背景items/fullscreen stateは編集しない。常時モデルレスでは複数本/複数窓との割り込みが増えるため採用しない。

入口は本のHUD「…」と本一覧の右クリック「本の構成を編集」。空/全非表示本でも入口を残す。
状態は一つの `CompositionEditorState = Preparing | Editing(draft) | Saving | Reloading`。
owner context/scope/requestは状態内に持ち、show bool + pending bool + draft Optionを重ねない。
共通modal helperとローカルopen admissionへ同じ**mount非依存のeditor owner**を登録する (§8.1)。
既存UI modal判定 (`src/app.rs:21897`, `:21912`, `:37865`) だけをRemoteの確定境界とみなさない。

画面構成:

- 見出し: 本の名前、構成あり/新規、右開き/左開き・表紙あり/なし。
  方向/表紙を変更できるdraft設定をここに置く。閲覧mode選択は構成の設定項目に入れない。
- 左: 元ページ一覧 (元名/物理番号、構成での非表示・Missingの表示)。元ページを見られるのはこの画面。
  file-type除外は採否policyを尊重し、必要なら名前だけ「ファイル種類の設定で非表示」と示す。
  この画面から種類設定を勝手に変えたり除外画像をdecodeしたりしない。
- 右: 編集中のunitカード。実ページはthumbnail付き、白紙は相方の実ページカードへ付随する
  半透明/破線の小さなplaceholderにし「この見開きの空き側」と表示する。実画像の白いページと区別する。
  白紙は単独選択・drag・前へ/後へ移動の対象にせず、相方実ページの操作で側変更/取り除きを行う。
  「白紙は見開きの組だけに使います。単ページ表示では飛ばします」を常時説明する。
  viewerでは既存§1.240の白い面を維持し、editor用placeholderの絵柄をviewerへ持ち込まない。
  drag並べ替えとボタン「前へ」「後へ」「非表示」「表示に戻す」「白紙を挿入」「この2枚を組にする」
  「単ページにする」「中央/片側」、元に戻す/やり直すを用意する。
  表紙を指定/解除し、選択Singleへ「表紙の補助表示」を付ける/外す操作も置く。
  末尾表紙/端配置/強制単独は自動フラグの一覧ではなく、該当unitの明示編集として見せる。
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
`with_window_viewer_context`による兄弟描画 (`src/ui_fullscreen.rs:22661`) でも
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
G0照合: 使用するlockは `SessionHandle.inner: Arc<Mutex<SessionStateMachine>>`
(`src/remote_ipc/session.rs:1219`)。別の `ui_request_admission` mutex (`:1225`) はRemote UI queue用で、
Local-phaseとの確定境界にはならない。private innerをAppから直接触らず、session moduleの短いhelperで
既存acquire (`:1544`) と同じinnerに入る。新helperは提案であり、現行APIに存在するとは扱わない。

lock順は **UI routerの直列処理 → session.inner → O(1)のowner/ticket採用 + try_send → unlock**。
App側owner/ticket tableはUI router所有で新mutexを付けない。channel full/disconnectedなら同じ短い受付内で
未送信ticketを外しEditingへ戻して通知し、write済みとしてbarrierへ数えない。
inner保持中にui_request_admission/gui gate/registry mount/service lock/DB/flush fenceを取得しない。
workerはsession/registryのlockを取得せず、完了channelを既存UI routerが収穫してからphaseを確認する。
既存BeginAcquireのgui gate publicationから逆順でinnerへ戻る経路を新設しない。
start/saveの非blocking受付と、保存workerが行うmanifest検証/sidecar準備/transactionを分離する。

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

## 9. §1.364との境界と編集入口 (R4決定)

E側と共有する問い合わせは `resolve_book_layout_source(scope)` (Book列に対するもの)。
構成が存在すれば Composed、なければ Automatic + §1.364の解決済shift。
ExtractedResultは本別shiftを持ち込まず常にshift禁止。OrdinaryListの従来一時shiftは、その元scopeの構成がない場合に維持する。
構成snapshotの取得に失敗した状態を「構成なし」としてshiftやbinding変更を許可しない。

`queue_spread_shift_action`の**Spreadで実際にずらすbranch**へ入る前に共通可否を見る。
ComposedまたはExtractedResultならそのshift意図を消費、anchor/target/cache/DBを変更せず通知する。
G0照合: 現行同handler (`src/ui_fullscreen.rs:28390`) のnon-spread branchは通常の一ページ移動。
`SpreadMode::is_spread` (`src/settings.rs:3096`) はSingle/Splitを含まないため、このfallbackをshift拒否より先に
通常nav意図へ分離し、Q16のtarget提示許可に通す。Action名だけで入口全体を禁止して単ページ移動を失わない。
Composedの説明は「本の構成で設定されています。組を変えるには本の構成を編集してください」。
ExtractedResultには「この結果一覧では見開きをずらせません」と説明する。元本の構成有無を調べない。
四つのFsSpreadShift Action、カスタムキー、ゲームパッド/リング等で同じ結果にする。
Single/Splitの従来page移動はshiftではないので維持する。
HUDの§1.364保存/解除も構成ありでは無効と説明し、保存shiftを消去する裏口にしない。

**Q1決定: 保存済shiftは休眠保持する。** 編集開始時の実効shiftを初期Pair/Singleへ取り込み、
構成保存後はsaved shiftを解決/適用/更新しない。saved row自体を消さないので、
保存跨ぎの二DB transaction/recoveryを必要としない。
構成解除時は§1.364の現時点の記憶ON/OFF・scope・identity解決規則で再評価する。
記憶ONかつidentityが解決できた場合だけ旧saved shiftを再び有効にする。記憶OFFなら休眠rowを
勝手にsessionへ持ち込まない。Missing等の旧anchorはEの採択規則に従い、同じraw番号へ代用しない。
解除確認ではその時点の実効結果を説明する。「必ず以前のずらしが戻る」とは表示しない。
作成キャンセルではshiftに一切触れない。記憶OFFのsession shiftも初期値に取り込むが永続shiftを新規作成しない。
saved shift消去の代案は撤回し、G側で§1.364の記憶規則を再実装しない。

### 9.1 構成ありのpairing操作はeditorだけ

R2/R3のHUD/キー/Remoteから構成recordを直接編集する契約は撤回する。
`PairingEditAvailability::Automatic | CompositionOwned(scope) | ExtractedReadOnly | Unavailable(reason)` を
accepted recipe/BookまたはOrdinaryListの元scopeから一か所で派生させ、UI表示とaction/setter/IPC受付が同じ判定を使う。
これは構成存在から得る操作資格であり、editorのPreparing/Editing/Saving等のphaseに連動する別boolではない。
兄弟contextの一時mount中にも、入力元contextまたはRemote addressのlogical scopeを使う。
非表示/絞り込み/別modeでも、構成の存在をAutomaticへ読み替えない。

| 既存操作 | 構成あり本の扱い |
| --- | --- |
| HUD方向/表紙、`FsReadingDirectionToggle`、方向/表紙を指定するSpread/Split系KeyAction、Remote `SetSpread`のbinding変更 | 拒否。構成の方向/cover/Pairを変えない |
| HUD末尾表紙、Remote `SetFinalCoverSpreadPreference` | 拒否。補助表紙を追加/解除しない |
| HUD先頭/末尾端配置、Remote `SetSingletonSpreadEndpointPreference` / 旧`SetSingletonSpreadPlacementPreference` | 拒否。placementを変えない |
| HUD表紙次/最終単独、`FsTogglePageAfterCoverAlone` / `FsToggleLastPageAlone`、Remote `SetPageAlonePreference` | 拒否。Pair分割/単独指定の変更をしない |
| 一ページずらし、§1.364保存/解除 | 拒否/無効表示。保存shiftは休眠のまま |
| 単ページ/見開き/横長分割の表示切替 | 許可。§9.2のmode-only操作で、構成を変更しない |

HUDはpairing関連の項目を消さず無効表示し、共通説明を**「本の構成で設定されています」**とする。
小さな「構成あり」indicatorを出し、tooltipで「この本は本の構成を使用しています」と説明する。
同じメニューに有効な「本の構成を編集」を置き、既存modal ownerからeditorを開く (§8)。
Remoteも同じ項目を無効表示し、同じ説明を出す。PCで編集する案内を出すが、Remoteからlocal editorを起動しない。
無効キーを別のpage移動へ読み替えず消費して通知する。再割当/ゲームパッドも共通handlerへ通す。

拒否はUIのwidget無効化だけでなく、実際の変更ownerで**保存前**に行う。
record/revision、spread.db、本別shift、全体/既定設定、共有resume、units/anchor/cacheは無変更とする。
同値のbinding要求でも成功した編集に見せず、同じ理由で拒否する。隠れた小編集writerを残さない。
全体/既定設定は構成なし本のために変更できるが、Composed resolverはそれを入力に使わず、
その変更だけで構成本の方向/表紙/unitsを再生成しない。作成時にも全体/既定値を書き戻さない。
お気に入り等の既存設定適用も、Composedへ方向/表紙/組設定を持ち込まない。無関係な表示設定の適用は維持する。
構成なしのBook/OrdinaryListのHUD/キー/Remoteと§1.364の機能は従来どおり。抽出列はA2の例外。

ExtractedResultは全列のbinding/shiftを無効にする固定資格。既存結合操作もview-mode部分へ明示分離し、
元本へのsetting setterを呼ばない。理由は「この結果一覧では本別の見開き設定を使いません」。
PCの既存Spread/Split系キーは抽出列ではSpread/Splitの表示切替として受け、direction/cover指定を保存しない。
HUDの表示方法と同じA2固定入力へ明示routingする。binding設定用のtoggle/shiftは無効のまま。
Remoteの既存結合SetSpreadはbinding変更の受付として拒否し、Webのmode switchはSetViewModeを使う。
この違いをKeyAction/IPC handlerテストで確認し、既存の見開きキーを押しても何も起きない退行を避ける。
OrdinaryListの元folderに構成がある時はQ12/13でも保存構成の外部変更を拒否する。
複数scopeを調べて資格を変える、構成なし部分だけ編集する第二ownerは不要。

### 9.2 閲覧modeをbinding変更から分離する

表示方法は `ViewMode::Single | Spread | LandscapeSplit` という意図として既存閲覧ownerへ送る。
構成のdirection/cover/unitsとは別で、構成record/revisionを変えない。ReadingFlowや端末orientationも従来の閲覧投影。
Spreadへ戻ると必ずaccepted構成の方向/表紙/explicit unitsを使う。Singleへ移ってもcover指定を解除しない。
LandscapeSplitの既存半ページ表示は§5.1を使い、方向を構成の値から解決してbinding変更を発火させない。
構成あり本のmode保存は既存の本別閲覧設定ownerに限定し、全体/既定の見開き設定更新や構成writeへ接続しない。
既存結合enumを保存/描画へ渡すadapterが必要でも、direction/coverはaccepted構成から供給する。
そのenumを構成の設定の正本や、方向/表紙変更要求として再解釈しない。
保存先は既存spread.dbの当該scopeのmode欄で、mode-only setterがその欄だけを更新する。
direction/flow欄、末尾補助/端配置/単独の各行、全体設定は変更しない。
Composedでmode欄から読むのはSingle/Spread/Splitの分類だけで、enumの方向/coverビットは採用しない。
全体値と一致したことを理由にrowを削除したり親scopeへ書いたりせず、対象scopeへ明示保存する。
既存結合setter (`src/spread_db.rs:407`, `:484`) が方向も書くため、そのままmode-only保存には使用しない。
構成作成時は現在の閲覧modeをcontextへ保持するだけで、このmode行への付随writeを追加しない。

現行 `SpreadMode` はdirection/cover/modeを一つのenumへ含む (`src/settings.rs:3082`)。
PCの `apply_fullscreen_spread_mode` (`src/ui_fullscreen.rs:37798`) とHUD受付 (`:25400`) は
結合値の保存・方向更新も行うため、構成ありのmode-only入口からその一括更新を直接呼ばない。
`FsSpreadLtr/Rtl`、cover付き、`FsSpreadSplitLtr/Rtl`は方向/表紙を指定する操作として拒否する
(`src/keymap.rs:1730`, `:1734`)。`FsSpreadSingle`はmode-onlyへ通す。
閲覧方法だけを選ぶHUD経路と、汎用の見開き/横長分割KeyAction (案: `FsViewSpread` / `FsViewLandscapeSplit`)
はmode-only意図を使う。新Actionは既存割当に衝突する既定キーを付けず、通常のkeymap登録/カスタム割当対象にする。
構成なしBook/OrdinaryListの既存結合操作は維持する。ExtractedResultはbindingを指定せずmode-onlyへ通す。新たな構成用mode storeやeditor外の構成command serviceは作らない。

Remoteの既存 `SetSpread` はmodeとreading_directionを同時に受ける (§2-10)。
構成ありでこの要求を保存することは拒否し、mode-only `SetViewMode` をIPCへ追加する。
Webの単ページ/見開き/横長分割切替はこの一つの入口を使う。方向/表紙のwidgetは無効のまま。
古い結合要求のbinding部分を黙って無視して成功にはしない。strict handshakeで新Web/IPCを揃える。
mode-only要求はBookでは構成snapshot、抽出列ではA2固定入力で表示投影を更新し、構成writerを起動しない。抽出列ではDBのmode-only保存も行わない。
Remoteの端末単ページ強制も引き続きview projectionであり、PC/他端末の本設定を書き換えない。
この分離は表示切替を維持するためのもの。表紙・方向や周辺操作の直接小編集APIは追加しない。

### 9.3 §1.364との接続契約

**2026-10-11 G3**: Eの採択済みC1–C12と共有`BookScopeKey`を取り込んだ。
`docs/spread-shift-contract-fixtures.md`と`src/spread_shift/contract_fixtures.rs`を正本とし、
Gは既存admission/restore ownerへ資格とidentityを渡す。Composed/Unreadable/A2ではshiftを休眠させ、
解除時は既存記憶ON・OFFの規則へ戻す。C7/C9のeditor固有の作成取消・保存はG4で接続する。
以下はG0時点の依存調査記録として保持する。

**G0の保留項目。Gはshift store/schema/保存ON・OFF時の解決規則/復元fallbackを設計しない。**
正本予定はラインE (`C:/home/mimageviewer-nextview`) の `docs/spread-shift-memory-plan.md`。
この時点ではその設計採択を待ち、下記はGが必要とする接続条件だけを列挙する。

| Eから必要な契約 | Gの確定済み境界 / 用途 |
| --- | --- |
| shift解決request/返答の型、採用世代、解決済shiftの意味 | `BookLayoutSource::Automatic(resolved_rules, optional_shift)` へ渡す。Composed/ExtractedResultでは解決・適用しない |
| 保存対象となるbook資格とscope key、logical/physical/prefix対応 | ZIP階層/元変換書庫/PDF/EPUB/画像folder/OrdinaryListの境界を確認。GのBookScopeをEのDB keyと同一だと仮定しない |
| anchor identityと当該manifest/nav orderへの解決、Missing/旧記録等の扱い | GはEから解決済み入力を受け、raw slotや保存値を独自に推測しない。Eが選ぶ復元規則/記憶ON・OFFに従う |
| editor初期化に渡す現在の実効shift snapshot | Preparing受付時のscope/items世代/自動規則と一体で捕捉し、shift適用後の**現在の組**を初期構成にする。保存値の再読込で初期値を変えない |
| 構成作成/保持/解除と休眠shiftのlifecycle | Q1: 構成作成/保存/失敗でEの保存shiftを消去・更新しない。解除後にその時点のE規則で再解決する。副作用のない境界/APIを合意 |
| 共通action admissionと返答/失効の順序 | Gの構成あり/Unreadable/A2拒否をEのshift変更**前**に確認。Single/Splitの通常nav fallbackは分離して維持。Ctrl左右/キー再割当/gamepad/HUD/再送も同じ入口。Eの新世代返答が旧projectionへ作用しない |
| rename/purge/content restore/import/resetの担当と通知 | Eのstore保守はEが所有。Gが休眠値を写したり別transactionで消したりしない。Gの構成revision通知とEのshift通知を混同しない |
| 共同受入fixtureと統合順 | 記憶ON・OFF/再入場/identity欠落、構成作成・取消・解除、兄弟窓、A2/OrdinaryListの期待値をEの採択後にG3へ具体化する |

記憶規則が決まるまでは§13 G3のshift関連acceptanceは契約待ちと明記し、G1/G2の非shift部分は分離して進められる。
上記はG0の不足設計をG内で埋める指示ではなく、coordinatorからE設計者へ渡す問い合わせ一覧。

## 10. F12 / 多窓 / Remote

### 10.1 ローカルcontext

DBサービスはApp共通、`PreparedBook/layout/identity map/revision/unit cache/editor draft`はcontext所有。
F12 forkはimmutable構成snapshotを共有できるが、nav/cache/queue/cancel/generationは複製したcontextのowner。
fork/park/mount/deposit/close自体はDB writeや sibling reloadを発生させない。
保存replyはrequestを発行したcontextへ配送し、今mountされているcontextを要求元と推測しない。

save後は**全保持Book projectionの依存scope**を照合し、同scopeの既存本reloadへ接続する。
anchor identity、mode、filter、return route、zoom/scaleを捕捉し、parked Bookもcontext identityで扱う。
Book anchorがHiddenなら§6へ着地する。別本/別階層は不変。旧snapshotは既存holdoverとして扱う。
ExtractedResult/OrdinaryListは構成保存による再取得・membership変更・nav失効・anchor移動をしない (Q16)。
結果列の非表示は次の新規提示時だけ照合し、現在一覧/画像/return routeを維持する。
無条件の窓close/zoom resetはしない。既存reloadでBookの状態を保てない場合は実装前に相談する。

#### 退避projectionの鮮度と復帰adoption

snapshot.saved_items、固定list_items、pre-search/return grid、F12元一覧、parked bundle、folder-historyの保持列も、
その**元recipeがBookかどうか**を保持する。Bookは
`{ recipe, accepted_projection, dependencies: scope→(composition_token, source_manifest_revision, policy_revision) }`。
構成なしも依存に入れ、作成/解除/再作成を検出する。別dirty boolや永続tombstoneは作らない。
source revisionはscopeの受入manifest/実削除/改名publicationで、contextのitems_generationを代用しない。
結果列の保持値には構成依存を追加しない。source/policy/membershipの既存鮮度確認だけを維持する。

`adopt_retained_projection` の契約:

1. Bookならscope依存が一致時だけ旧immutableをinstall。不一致なら保存recipeから既存reloadし、
   新投影でidentity selection/anchorを復元。Hiddenなら§6の旧各窓順で着地する。
2. 結果列なら構成保存を理由にreloadしない。固定membership/結果順/return routeをそのまま復帰。
   grid復帰では構成照合ゼロ、fullscreen復帰で新たに出すanchor/相方だけ遅延照合し、Hiddenなら§6へ。
3. どちらも復帰adoption/背景更新から共有resume/historyを新規記録しない (§6.1)。

固定解除 `snapshot_ops.rs:1282`、固定一覧帰還 `:1406`、F12元一覧 `viewer_context_registry.rs:3418` を含め、
Bookの旧items直接代入で旧構成を復活させない。結果列へ新しい構成更新callbackは登録しない。
Book再取得中は既存loading/holdover、固定membership/source label/pre-search origin/return routeを破棄しない。
G0照合: snapshot/F12の直接代入は統合後も残る。`adopt_retained_projection` は新しい共通接点として実装が必要。
Eが追加した `fs_continuous_fit_reference` / retained PDF metadata / captured fit inputも各bundleの寿命に従う。
新Book投影でidentity/role/slot/sliceが変わるidx参照は旧fit参照ごと採用せず、受入済み新unitでEのresolverを使う。
同じ投影のread-only復帰/forkでは現在のfit入力/手動zoom/panを保ち、Appのlive sibling値から再生成しない。
receiptはApp共通storeにだけ保持し、復帰recipeやF12 bundleへ載せない (`330e070f6`の訂正を維持)。
本から離れている窓をsaveで強制移動させず、退避Bookは復帰時に更新する。

editor terminal closeは §8.1/§10.2に従う。indexを使うdraftを別窓に移植しない。
detached述語/viewportに触る前にはClaudeCodeと独立Codexレビューで
「context投影の所有を拡張する構造的修正であり症状パッチでない」ことに合意し、
実装範囲と理由を `detached-rework-plan.md` §11へ記録する。
新規detached bool、geometry捕捉、repaint/retry/time窓による修復は不要。

### 10.2 Remote

coreのContainerEngine workerが同PreparedBookを生成する。Webは返されたentries/PageGroupを表示し、
構成のorder/pairを自前で再現しない。実addressは元root+entry/物理PDF・EPUBページ番号のまま。
Bookには `has_composition`、revision/accepted layout identity、構成の実効bindingを追加する。
操作資格と「本の構成で設定されています」の理由も返し、HUD/Remoteで同じ無効項目を表示する。
ExtractedResultには列種別/A2の固定操作資格/一時modeを返し、leaf hidden marker/構成依存を送らない。
結果のthumbnailはそのまま、full-page/ナビ候補をcoreで遅延照合してskip/拒否する。列種別から生成し、
既存presentation/anchor/singleton placementを共有する。白紙のaddressを発明しない。
件数/response budgetで切詰める前にfull projectionを作り、切詰め後はcomplete=falseとする。
Pairの片側が応答外なら残る実ページのslotを保ち、見かけの末尾へ表紙を付け足さない。

構成editorの全体編集は今回はPCのみ (Q6決定)。Remoteは構成で読み、非表示を拒否する。
方向/表紙/末尾表紙/端配置/強制単独/一ページずらしは無効表示し、変更要求はcoreで拒否する (Q2/11/17決定)。
単ページ/見開き/横長分割の切替は§9.2のmode-only入口で維持する。
Remoteの全体編集UI/APIの追加は別段階/要求とし、本機能のPC編集と同時に作らない。
構成なし本の既存書込はIPC→core所有service。read-only connection/workerから書かない。

#### Remote取得は全ローカル窓closeへeditorを含める (Q9決定)

現在の「後から操作した側が勝つ」を保ち、editor中でもBeginAcquireを拒否しない。
`poll_remote_session`のAcquiringRemote分岐 (`src/remote_ipc/ui.rs:1081`) の全窓closeに
editorのterminal closeを含める。`close_all_detached_viewers_for_mode_change`の
`had_detached == false`早期return (`src/app.rs:58330`) やfullscreen_idxなしでeditorを見落とさない。
全ローカルsurfaceのclose coordinatorがglobal editor ownerを無条件で列挙し、
context close関数は同じterminal処理を使う。別modal窓だけ残す設計にしない。
`local_viewer_close_state` (`src/app.rs:74141`, `:74170`) はeditor owner/viewportと
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
失敗は旧record保持と保存失敗の通知。消えたdraftを復旧・自動retryしない (Q7/Q9決定)。
Reloadingは保存済recordを維持し、local reload/返答をretire、Remoteが新snapshotを列挙する。
service drainは既存AcquiringRemoteのclose barrierへ接続し、Remoteの最初のenumerationは
保存writeが終端になった後。既存read workerは `wait_until_active`
(`src/remote_ipc/pipe.rs:1797`) を通り、AcquiringRemoteはそこで待つ
(`src/remote_ipc/session.rs:2501`, `:2524`)。write workerも同じ待ちを持つ (`src/remote_ipc/pipe.rs:1338`)。
この既存activation境界を使用し、取得応答がActiveというだけで列挙を先行させない。
G0照合: 現行 `remote_ipc/ui.rs:1081` は窓close/AI barrierだけでFinishを判定し、editor/ticketはまだ存在しない。
この同じacquire barrierのFinish条件へ **mount非依存editor ownerが終端 + 送信済save ticketが終端** を追加する。
ticketは閉じたcontextのdraft内ではなくApp共通worker完了管理に残り、全窓closeで脱落させない。
`local_viewer_close_state` (`src/app.rs:74141`, `:74170`) だけがClosedでも未完了ticketを見落とさない。
`finish_acquire` (`src/remote_ipc/session.rs:1656`) はbarrier完了後だけ呼び、Remote read/writeの既存
`wait_until_active`をそのまま使う。新しいRemote取得拒否条件や二つ目のsession ownerは作らない。
現在の30秒取得失敗処理 (`src/remote_ipc/ui.rs:1125`) は維持し、
追加の待ち時間/retry/保存完了後のlocal窓復元を作らない。
owner窓終了/Local返却/取得abortでも旧requestを新ownerへ配送せず、replyはserviceで完了させる。
app終了途中のcommitはDB一transactionの従来の保証に任せ、永続draft回復は要求しない。
SessionHandle mutexを取った短いUI確定処理からservice状態のblocking lockを取らず、
worker側からregistry/sessionへ逆向きlockを取らない。通知/完了はchannelでUI routerへ返す。

編集中の取得拒否案は採用しない。後のRemote優先と送信済save barrierはQ9で決定済み。

PC変更の端末反映は既存session acquire時のcache破棄で扱う (`web-remote-plan.md` §2.2/§12.16)。
全editorをPC限定にしRemoteには構成writeを持たせないので、新data_version watcherは要らない。
Remote構成小編集のcommit/reply/revision通知と、それを理由にしたcontainer/group cache失効は撤去する。
mode-only変更、force_single/端末orientationは構成revisionを増やさず端末ごとのprojection keyへ載せる。
mode変更に必要な既存remote generation/cache失効は閲覧ownerから行い、撤回した構成write通知とは分ける。
構成なしの既存設定変更に必要なremote generation/invalidation通知は維持する。
IPCは調査時v69から統合時の最新+1、Web asset tokenとstrict handshake/testを一緒に更新する。

## 11. トリム・変換書庫・出力の契約

### 11.1 view-trim spread_separate

本別 `spread_left/right` は画面の側への設定として保持し、ページidentityへ焼き込まない。
pairを変えてページが反対側へ移れば、その新しい側のmarginが掛かる。RTL変更も同じ。
page overrideは従来どおりページ固有で、本設定との優先順位は `view_trim::stored_view_trim_bbox`へ集約する。
構成保存でmarginをswap/average/resetしない。crop/mask/annotation/edit空間も変更しない。

Single+blank/片側配置も**spread閲覧中のslot側**で本別spread marginを選ぶ (Q8決定)。
普通のSingle閲覧/編集pivotはsingle margin。blankにtrim/auto-trim/partner decodeを要求しない。
Autoの二枚上下harmonizeは実partnerがあるPair/補助だけ。
現行Single rendererはsingle bboxを使う経路があるので、単なるSpreadPair::Singleへの変換だけでは不十分。
slot/presentation modeを共通trim resolverへ渡し、local/Remote/continuous/holdoverで揃える。
§1.240既存singletonにまでこのtrim変更を拡げる場合は既存仕様との相違を別途相談し、
構成branchの追加に便乗してreleased本のtrimを変えない。

### 11.2 変換書庫

`logical_root` (元RAR/7z/LZH等) と `physical_read_target` (cache ZIP/直接RAR) をsnapshotで分離する。
current_folder + archive_source_overrideが同じ受入済cacheを指す場合だけ対応を使う
(`src/app/snapshot_ops.rs:2242`)。App globalなcache aliasを別contextへ当てない。

- 構成/構成PageRef/しおり/新target lookupは元root+entry。
- デコード、既存回転/補正/trim等page DBは現行物理cache側のキーを維持。
- 自動spread値を作成初期値へ取り込む時だけ現行cache側 `spread_container_key` を解決する。
- 再変換後cache path変更でも、entry identityが同じなら同じ構成へ解決する。
  entry名/階層が変わればMissing/新tailであり、items番号から推測しない。
- 元書庫が消えても受入済cacheのF5閲覧は現行どおり維持し、既存overrideから構成を引く。
  source currentnessを新たに毎open必須にしてcache-only閲覧を退行させない。

EPUBは書庫のsource/cache entry mappingとは別で、元EPUB+page_numと既存ReadTarget/leaseを使う (§4.1)。
cache削除→同条件再変換でもDB/sidecarへ世代pathを書き込まず、構成を同番号へ再解決する。
content identityは元EPUBの既存kind/固定世代provenanceに従う。再変換で内容がずれた時の自動救済は追加しない。

### 11.3 書き出し・製本・metadata bundle

通常一覧の一括書き出し/製本追加は `grid_selection_indices` が返す構成後index昇順。
構成非表示はitemsにないので選ばれず、白紙/補助もsourceに変換しない。
結果列のサムネ/selectionにhidden markerは作らない。明示したselection→実Source確定時にだけ
workerで共通visibility resolverを通して非表示を除外/通知する。これは列生成/保存時refreshとは別の出力受付。結果の可視選択は従来の出力順を維持し、
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
| 異なる本の明示Pair / 抽出列自動pair / 動画の混合layout | BookだけComposed、ExtractedResultは固定Automatic一つ。全leafの構成依存を作らない |
| 任意page/blank列 + explicit pair overrides + singleton flags | explicit units一つ。flat列は派生、blankはsourceではない |
| 自動landscape/page_alone設定変更 + 保存済手動Pair | 初期値で自動規則を凍結。以後はgeometryだけ更新 |
| 閲覧mode/orientation/Splitが本の永続ペアを変更 | view projectionを分離。single/splitは同じ可視flat列 |
| 保存中のitems in-place並べ替え + index編集cache | draft→commit→既存reload。削除等は共通remapでitems/units/identityを同時更新 |
| 新構成で表示中 / ★固定・F12退避に旧構成 | 保持recipeとrevision照合、一つの復帰adoption。固定membership/return routeは維持 |
| 個別本reloadとF12独自rebuildを別々に実装 | 既存context付きreloadへ接続。scopeに関係するcontextだけ更新 |
| 読取open/closeでmutation invalidation | readはrevision採用だけ。実際のwrite/rename/importだけ通知 |
| 書庫cache keyと本のidentityの混在 | logical/physical typed snapshot。ページ編集DBは既存keyのまま |
| 元書庫構成 + cache spread/trimの転送origin競合 | composition専用logical family。既存container origin判定へ参加させない |
| Folder内容ID / 一枚の画像ID / 類似pathによる親構成推測 | G4Fはfolder内sidecarの不足行自動補充と明示bundle。Folder hash/復元元選択UIなし |
| HUD/キー/Remoteの構成小編集とmodal editorの二つの変更入口 | 利用者決定でeditor一つへ集約。小編集command/transaction/保存replyを撤去し、外部入口は共通拒否 |
| 外部cover/方向変更と手動Pair、旧Pair復旧log、同値小編集再送 | draft内の明示編集だけ。editor外の自動Pair分割/復帰、no-op再送処理、unit対象選択resolverを作らない |
| 兄弟窓reloadの完了順 / 共有resumeの優先順位 | 窓内anchorと保存要求のresume intentを分離。保存完了ownerから最大一回、退避/parked更新は書込なし |
| 末尾補助/端配置/強制単独の自動フラグ + 明示unit | 初期化時だけ取り込み、以後はeditorのexplicit unit。外部toggle/FollowGlobal変換commandを撤去 |
| 全体/既定binding + 構成bindingの再適用・書戻し | Composedのresolver入力は構成のみ。初期化snapshotは全体値を変更せず、他本への波及をなくす |
| Remote構成小編集commitとgroup cache/revision通知 | Remote構成writeを撤去。PC保存は取得barrierと既存acquire refresh、端末modeはview projectionだけ |
| standalone Blank / BlankId / BlankBoundary / 白紙だけの編集選択 | 白紙は実ページSingleのcompanion値だけ。相方なしはdraft操作/保存で拒否。blank-only閲覧/移動状態を撤去 |
| 結果一覧hidden marker / 画像を隠す別thumb / scopeごとのhidden cache | 通常thumbnailをそのまま表示。新規提示する対象だけworker照合。列生成lookup/marker payloadを撤去 |
| 構成保存後の全結果reload / stashed結果再prepare / leaf構成revision依存 | Bookの依存だけを更新。結果membership/return routeは不変、grid復帰は構成照合なし |
| 結果内の構成本有無とshift可否の組合せ | A2は常にshift/bindingなし。全leafの資格走査・部分shift・現在anchorによる資格切替を撤去 |
| 抽出一覧の永続mode / per-list spread key / 再入場時の旧mode | modeはview sessionだけ、新規openはSingle。synthetic keyのread/write、旧mode適用と解除migrationを追加しない |
| EPUB世代cache→元本→独自page remap | 既存個別編集と同じ元EPUB+page_num。再変換でずれる制限を明記し自動救済を作らない |
| folder構成専用backup toggle / 第二sidecar writer / 復元確認prompt | G4Fは既存backup設定・SidecarFile owner・不足行自動補充だけ。書庫/PDF/EPUBから分離 |
| items flushとroot flushの別dirty/世代/完了 | SidecarSnapshot一つをPendingWriter/flush予約/import/reuseへ渡す。空判定もrootを含める |
| mtime秒値の消費 / edit同期が構成同期を兼ねる / タグwriteごとのDB callback | 同じstrict engineの独立構成family。実disk token + root消費witness一つでタグだけの変化を判別。token履歴/全folder削除tombstoneなし |
| backup失敗でDBを戻す / DB成功とfile成功の二段回復 | 利用者採択: DB結果保持 + 通知。解除の消費receiptとbackup writeを分離し、自動retry/同時rollbackなし |
| nav anchor移動の許可 / 連結viewportの許可 / cached textureの暗黙許可 | 既存display demand ownerへ候補単位許可を載せ、提示projectionからpaint/geometry/traceを一体生成。prefetchとは分離 |
| Hidden対策の連結無効化 / 全結果の事前hidden走査 / 永続skip bitmap | 連結の可視新入場だけ照合、要求内だけskip。再入場で再確認、保存時refreshなし |
| フォルダ例外の修正が共通Book完成を止める | G4Fは独立採択。難航の相談点を固定し、ZIP化の回避で共通部分を保ったまま切れる |
| 新storeの移行/旧設計試作データ互換 | 未リリースstoreは新schema一つ。released既存DB/bundle互換は維持 |
| 稀なDB failureのretry/途中保存/rollback後のrollback | record一transaction + draft維持/通知。独自回復網は利用者相談前に作らない |

採用しなかった簡略化: 構成保存だけを理由に全窓を閉じる、全設定reset、PDF即時表示を全件待ちへ変える、
構成なし本のshiftを取り除く、読み取り時の新tail自動保存。いずれもreleased機能の退行、
利用者データ変更、または別contextへの不要な副作用がある。
Remote取得の全窓closeはそのreleased契約を維持する場面であり、保存更新の全窓closeとは別。
構成あり本のHUD/キー/Remote操作制限は利用者が明示採択した変更で、追加承認待ちに戻さない。
抽出一覧A2、本扱いOFF/混在のOrdinaryList、Q16の遅延判定は承認済み。結果列の従来mode記憶/表紙/shiftを維持する互換branchは残さない。
view-modeだけをbindingから分離する入口は残す。これを削ると利用者が維持を指定した表示切替まで失うため。
連結読みの禁止も採用しない。anchor移動だけでなく既存可視候補ownerへ照合を接続すれば、閲覧機能を維持できる。

## 13. 段階・担当範囲・受入テスト

設計担当が下記chunkを確定し、各chunkのwriterを一人にする。実装と独立レビューは別context。
本書では追加subagent/レビュー実行をしていない。設計handoff後にcoordinatorが独立レビューを手配する。
E §1.342の統合前提は `fa280d0d0` / `76488f0ec` で充足。§1.364の契約待ちは§9.3に分離し、Gではshift storeを設計しない。

### G0: 設計採択・E統合後の再照合

成果: R6独立レビューACCEPTと `330e070f6` の訂正を保持。§14の1〜17とbackupだけの保存失敗は全て決定済み。
今回のG0ソース照合とE接続の更新は以下。shift-store contractは§9.3の明示保留、detached §11案はhandoffにだけ記載する。
受入: §2のsymbol/lineをmerge済masterで更新し、Eのgeometry/scale/unit/anchor差分を確認。
editor start/save-ticketとsession mutexのlock順/全close barrier、Book復帰adoption、
一つのresume intent、Q16の候補単位admission/連結viewportの提示許可、A2のview-mode境界を構造レビューする。
G4F採択時は全sidecar snapshot/予約/reuse、内容tokenとroot消費receipt、明示空反映との接続を構造レビューする。
R6の実際のACCEPTと、今回の統合後変更に対する次レビューを分ける。次レビュー/設計担当確認前にG0全体を採択済みとしない。
小編集/EPUB延期/連結読み制限を再導入しない。shift契約待ち以外の今回照合はソース上で完了、実行観測はない。

依存順: **G1 (既存列の統一/A2分類) → G2 (pure model/fixture/共通store) → G3 (本への適用/遅延判定)
→ G4 (editor/多窓) → [G4F (画像folderのみ、切り離し可)] → G5 (最終統合)**。
G1の完了条件へG2以降の構成fixtureを先取りしない。G4F未実装でも書庫/PDF/EPUBのG1〜G4/G5が完結する。
共通stagesは画像folder列挙/編集/表示を従来どおり維持するが、folder構成の新規作成/保存/restoreはG4Fだけ。

### G1: items列生成の統一・抽出一覧A2の境界 (構成なし)

所有: app/book_materialization、ZIP/PDF/EPUB/しおり/snapshot/Remote producer、列種別分類。

**2026-10-10 coordinator決定**: order一致は、構成が適用される各本階層の直下ページblock内に限定する。子階層（subfolder/nested archive）のカテゴリ位置はRemote通常一覧・評価履歴経路を含む各surfaceの現行順を維持し、released順序を変更しない。構成は一階層のページだけを並べ替えるため、子階層の位置までsurface間で統一する必要はない。

**2026-10-10 coordinator決定**: A2から元の本へ戻るときは、変更前と同じ本の閲覧状態を復元する。元列と閲覧状態を既存snapshot ownerへ一緒に退避し、同じreturn adoption内で両方を戻す。別owner/第二commit点は作らず、A2のSingle/一時Spreadを本へ書き戻さない（A2「記憶しない」の復帰契約）。BSで固定列へ戻る場合はA2、新規openはSingle。検索のback/BS・履歴の本への復帰は既存Book設定adoption、F12はcontext内snapshot/bundleの復帰、Remoteは本の通常response復帰へ同じ契約を適用し、各境界を回帰testする。

**2026-10-10 G1 review修正（P2-1）**: 連結方式（flow）はA2のpairing初期化と分離し、各入口の従来の採用規則を維持する。タグ・お気に入り・評価・サブ展開・Smart新規rootは既存の保存flow（旧Vertical表現を含む）または従来の分類に対応するdefaults（通常はNON_BOOKのPaged）を採用し、GlobalSearch・Smart保存root復帰・Collection root・固定snapshotの直接列差し替えは既存flowを維持する。単ページ化・再openだけを理由にflowを変更せず、連結読みの操作を維持する。旧Vertical mode行はflow列に別値が残っていても従来どおり縦連結へ復元し、旧行は変更しない。

受入:

- 数字名/日付/評価sort、通常folder/製本、PDF/EPUB、wrapper/nested/直接RAR/変換cacheで元manifest順一致。
- ZIP root/enter/back/DFS/評価ZipDir/bookmark/snapshot/Remote list/page contextのscope/order一致。
- policy→分類、same-name除去、cached document早期adoption、password/EPUB conversion cancel、旧worker不採用。
- A read-only open/close/forkがBのitems/generation/queue/cancel/cacheへ変更を出さない。
- items/metas/visible_indicesの対応、complete proof/Remote切詰め、remove_items_batchの従来remap。
- 全A2入口を同じExtractedResultへ分類。一冊分の検索/複数本/動画混在でも新規open=Single。
  一時Spreadは別本の隣接画像を組み、cover/shift/末尾補助/端配置/強制単独なし。
  synthetic keyに古いSpread rowがあっても無視、全体値Spreadでも初期Single、mode切替/既存Spreadキー/再openでDB writeゼロ、binding toggle/shiftは無効。
- history/bookmarkからBookへ入る経路、通常folder詳細sort、通常mixed/音声/動画の既存閲覧はA2へ誤分類しない。
  ★snapshot一致による直接閲覧と固定membership/return routeを維持。構成fixtureはG2/G3へ置く。

### G2: pure model / 共通store / lifecycle / 転送 (書庫・PDF・EPUB)

所有: pure model/resolver、単一store、rename/purge/content restore、metadata transfer。

**2026-10-10 G2 part A実装範囲**: `src/book_composition/{model,resolve,edit}.rs`。
`BookComposition`はPair/Singleのみ、BlankはSingleCompanionの値であり独立page/IDを持たない。
hiddenは登録済refの集合指定、Missingはmanifest不在から導出する。resolverは寸法・自動規則・shift・
App/UIの入力を持たず、元slotの欠落Singleと新tail中央Singleを生成する。saved recordは変更しない。
`ResolvedComposition::remap_removed_items`はrowsと同時採用するold→new mapと削除identityを受け、
unit/anchor/helper/target/実ページflat列をまとめてremapし、登録済みの削除refをMissingへ更新する。
不可視refはpresentation statusにrow indexを持たないため、Hidden/policy/facetの実削除も
必須のidentityで更新する。可視ページのidentityとmapの削除指定が違えば操作全体を拒否する。
未保存tailの削除はstatus/tailから除き、残るfacet状態からcompleteを再計算する。
G3は同じtransactionで元manifest/identity表も更新し、永続refを削除しない。
resolverのindexは入力manifestのrow空間を維持する。G3で構成順のrowsを作る時は
`reindex_items`でtarget/unit/anchor/helperを同じ新slotへ写してから一括採用し、
並べ替えの未採用row空間をviewerのitems indexとして公開しない。
reindexの出力は旧rowsの並べ替え/絞り込みだけで、新rowsを追加しない密な列
`0..count(Some)`とする。旧row数以内でも出力範囲外/穴/重複は拒否する。
`fixtures::four_page_book` / `editor_unit_edits` / `hidden_blank_deletion_book`はG3/G4の再利用用pure checkpoint。
draft操作はcloneに適用→同じvalidatorで検証し、拒否なら元draft/revisionを変更しない。
coverを含むPair作成は明示`clear_cover`と補助除去を同じ操作で行える。

schema 1、通常参照/manifest/解決後参照は各100,000件、参照文字列32,768 UTF-8 bytes、
JSON 16 MiBを上限とする。JSON入出力とdraftで同じ検証を使い、未知字段/variant/schema、
unsafe relative/entry、種類/直下scope不一致、通常occurrence重複、cover/補助/hidden不整合を拒否する。
lexical安全検証でFSのreparse-point/containment検証を代替せず、safe raw manifestはsource workerの責任。
書庫memberはbytes参照なのでWindows file名の禁止文字を流用せず、relative/traversal安全性と直下scopeを検証する。
`[A,白紙,白紙,B]`はdirectionと無関係に保存unitの読み順slotで拒否する。
Hidden/Missingに付くBlank指定は保存モデルに残す。

保存キー`BookScopeKey`はラインE §1.364の`src/book_scope.rs`が所有する。
G2Aは種類/階層の検証用`PageDomain`だけを定義し、root/key/storeを複製しない。
G2 part BはE統合後。store/rename/purge/content restore/Edit ledger/transfer/sidecar/runtime接続は本chunkに含めない。
JSON round-tripはpure memory内の検証であり、DB transaction rollbackの検証を済ませた意味ではない。
自動検証のcommand/件数/exit/logは`target/G-1242-g2a-msg.txt`へ記録し、独立レビュー/製品実機確認は主張しない。
G2A REVISEの境界修正・fixture・G1入力テストの共有native frame所有・master照合と
再検証結果は`target/G-1242-g2a2-msg.txt`へ記録する。最初のfull-lib失敗はstartup restoreと
G1 extracted key handlerであり、video EOF/saved-group modalの失敗として扱わない。

**2026-10-11 G2 part B初回実装記録（249dc75a0。REVISE修正は下記）**:

- `book_scope::BookScopeKey`を共用し、新DB一つ・schema 1の提案四列を使用する。JSONは`{ domain, composition }`。
  empty構成にも種類/実効prefixの検証を持たせ、root/cache pathはpayloadへ入れない。revisionは受入先writerが実変更時だけ進める。
  書庫domainのprefixは同じ`book_scope::normalize_level_prefix`で比較用に正規化する。
  大文字/区切り表記だけが違うPrepare/Saveは同じscopeで、実memberのraw spellingは保存・読取とも保持する。
  有効なraw表記の保存行も無書込で読め、同値Saveでrevision/token/通知を進めない。
- `BookCompositionService`のPrepare/Save/Remove/root rename/purge/page移行は単一FIFO worker。
  `Arc<CompositionSnapshot>`とscope別`(service_epoch, publication_sequence)`、実変更keyだけの通知を公開する。
  service再起動・解除再作成もtokenを区別し、read/noop/open/closeではDB writeやscope通知を出さない。
- 既存maintenanceはEPUB→shift→compositionの順でquiesceする。UIはjoin/DB/FS/maintenance待ちを行わない。
  long-running import等は既存modalを使用し、新しい割込みrollback機構を作らない。
  まれな失敗はログ＋通知。保存transaction失敗は旧record保持、読めないstoreは保持・書込停止、commit後reload失敗はDBを保持して通知する。
  editor draft/今回だけ無視/実投影の失効はG3/G4のownerへ接続する境界であり、G2にeditorを作らない。
- `STORES`へKeepDrive/unique/logical family登録。generic root/prefix rename/copy/probe/purge/cleanup、内容IDcopy、
  個別page相対ref移行、明示settings完全resetへ接続する。resetは既存一回の確認へ構成解除の説明を追加し、
  workerで検証済み構成DBの退避snapshotを作ってから解除する。未知storeはsettingsを含め変更前に拒否する。
- 成功Saveは要求時に捕捉したlogical outer rootへEdit観測。兄弟mount/current_folderに依存しない。
  PDF/EPUB/ZIP/変換元書庫の構成だけの保存でも観測し、Folderは除外する。importはcommit済み捕捉rootの集合を同じobserverへ渡す。
- metadata出力は統合時v10→v11。composition専用familyをsource KeepDriveのlogical entryへ置き、cache由来の従来spread/trim originを変更しない。
  v7–v10/section不在は保持、v11の空はcoverage内解除、非空はlocal revisionで置換。coverage外/skipは保持。
  previewの追加/置換/解除と注意文は既存ImportConfirm一回へ追加。cancel/項目失敗は従来batch/SAVEPOINT規則を維持する。
- runtime readのaccepted recordはviewer bundle/mount/fork/park/snapshotへ所属させ、Remote SessionHandleへ同じserviceを登録する。
  Bookだけ新recordを読む。A2はlookup/write/refreshしない。retained snapshotは復帰時に新tokenを照合する。
  G2はstorage adoptionまでで、G3の構成順rows/builder/hidden admission/Remote wire payloadを実装済みとはしない。
- §7.4の画像folder sidecarはG4F限定。G2では既存sidecarのversion/items/writer/receiptを変更しない。
  storeは現在のdata-dirへ固定し、次の起動で指定data-dirから開く。保存先一式を運ぶ際は新DBも同じ場所に含める。
- command/count/exit/logと実際のfull-lib失敗は`target/G-1242-g2b-msg.txt`。settings transferはpreferences専用で構成を含めず、実テスト件数を同handoffと関連docへ記録する。

受入:

- Pair/Single/Blank companion/補助/hidden round-trip、unsafe/重複/未知schema/上限拒否、transaction rollback。
  standalone/blank-only/二つ目のblankと `[A,白紙,白紙,B]` のunit境界連続も拒否し、Hidden/Missingの実参照に付くblankは保持。
- model導入後に `[A,B],[C,D]` のB削除remap/Missing、editor内unit編集fixtureを導入。G3/G4は再利用する。
- root/prefix rename、book削除/page削除の違い、KeepDrive衝突なし、content copy先既存構成保持。
- 構成だけ保存 (閲覧/履歴なし) → PDF/EPUB/ZIP/元変換書庫を外部移動 → Edit ledger/内容ID復元候補。
  reply時に兄弟mountでもcaptured logical rootへ記録。Folder ledgerを追加しない。
- EPUB個別編集keyと構成refの同値をsource path/page_numで証明。generation/cache path変更でも同値。
  再変換で番号の内容がずれたfixtureは同番号へ解決し、救済mappingを作らない指定仕様を検証。
- source KeepDrive構成 + cache DriveStripped spread/左右trimをroot/nestedで転送往復。既存origin競合は誤発火しない。
- 新format構成あり→replace、新format構成なし→clear、旧v7/v8/v9/section不在/範囲外/skip→keep。
  部分coverageは他prefixを消さない。複数本追加/置換/解除のpreviewは一括確認一回、本別promptゼロ。
  cancel/項目失敗の既存batch/SAVEPOINT規則を維持し、resume/履歴を転送しない。
- readでDB writeなし、cleanup/reset/保存先/F12 bundle/Remote登録、実変更scopeだけ通知。

**2026-10-11 G2B REVISEのread境界修正**:

Prepare応答のscopeだけでなく採用直前の最新snapshot tokenを照合する。旧応答は最新Prepareに置換し、
通知消化済みの退避contextもPreparingの既存poll対象へ残す。構成serviceのdrainは描画pass後の
display-independent更新境界へ移し、mounted async ownerはAutomaticからの読取とAcceptedのscope照合も行う。
fullscreen中の新規open/本切替/同scope保存でも読取を止めない。
Preparing中だけ既存pendingの遅延repaint契約でROOTを起こし、応答採用時は一度repaintする。
retained contextの完了も入力なしで回収する。タイマーはworker完了のpoll用であり、競合の時間窓やretryではない。
新しいApp状態・worker側UI owner・DB/FSの同期処理は追加しない。G3のunit/hidden投影はこの修正に含めない。
簡素化は既存のasync境界・Preparing状態・pending起床に一本化する形を採用した。
背景readのためにopen/本切替をmodalで停止する案や窓を閉じる案は、通常閲覧の動作を変えるため採用しない。
旧Prepare完了→保存→通知drain→退避応答採用の順、fullscreenのscope lifecycle、入力なしの完了と
pending終端後の起床停止を回帰fixtureで固定する。検証実数/exit/ログは`target/G-1242-g2b-fix-msg.txt`に記録する。

### G3: 構成適用・spread/位置・§1.364・遅延非表示

所有: common projection、builder/cache、resume/target resolution、continuous表示要求/許可resolver、Eとのshift API。
受入:

- 初期snapshotを旧自動builder oracleへ照合: LTR/RTL×cover×1/2/3/4/5/多数×landscape/non-pairable×
  §1.239両端×§1.240両切替×末尾補助×実効shift。全体/既定/spread.db/保存shift/別本は作成成功/取消/失敗で不変。
- `[A,B],[C,D]`→B非表示/再表示/順交換、Single+blank、全非表示、Missing復帰。
  新tailは元順の中央Single、既存Pair/補助は不変。解除→再作成はその時点の自動初期値へ戻る。
- explicit Pairは回転/寸法後着/全体既定変更で組み直さず、Single/Spread/連結/Split/portraitSingleで同じ実列。
- B実削除直後のunit/anchor/resume/出力をF5前に検査。再読込後もB参照Missing、AとCを組まない。
  改名/外部追加/種類policy/prefix変更はidentityで再解決。
- resume identity/旧raw-only/trigger/generic migration、history ordinal/bookmark hintが別ページへ誤着地しない。
- 全pairing action/HUD/setter/Remote bindingを構成ありで拒否し、mode-onlyは維持。
  Single/Split/絞り込み/兄弟mount/同値要求もDB/revision/anchor/cache/resumeを変更しない。
  保存shift休眠、解除時は§1.364の記憶ON/OFF・identity規則。構成なしBook/OrdinaryListは既存操作維持。
  Ctrl左右等のSingle/Split fallbackは通常ページ移動として維持しhidden targetを照合。Spreadのずらしだけは拒否し、
  fallback移動で方向/表紙/構成/shift storeを変更しない。§1.364の採択済み保存fixtureを§9.3の境界で使う。
- 結果生成/thumbnailに構成lookupゼロ、保存で結果reload/marker/依存token失効ゼロ。
  hidden画像も通常thumbnailで残る。クリック/required snapshot/しおり/評価/履歴の明示targetは直前lookup→toast拒否。
- 単ページ/見開き/連結/slideshowの次候補だけhidden確認、相方も確認、片側Hidden→元slotSingle、
  両側Hidden→次unit、全Hidden→一覧。非表示解除後の次提示では再確認し、永続skip cacheを持たない。
  取消/世代変更/構成変更後着のadmission replyは旧targetを開かず、UI同期I/O/全列lookupを追加しない。
- 連結でanchor=Aのまま、prefetch/processed/raw/thumbnail cacheにあるHidden Bがscrollでviewportへ入る。
  BのChecking中/Hidden確定後に画像paint/transform登録/hit test/読了traceはゼロ、許可されたAは継続。
  Bをskipして次候補を限定照合し、先行anchor変更/空段/他unitとの再pair/全列lookupなし。
  wheel/drag/gamepad/slideshow/resize/scale/寸法後着を同じ可視候補受付で検証する。
- Single→Spreadで相方Bが初めて提示対象になる (pagedと連結の両方)。Bがcachedでも許可前は描かず、
  B Hiddenなら元slot Single、両AllowedならPairをatomic採用。Split/補助の新occurrenceにも同じ規則。
  当該結果窓が別窓の構成保存でrefreshされていないケースを含める。
- 連結の継続表示は保存時に回収せず、退出→再入場の許可を新しく取る。Hidden→可視変更後も再入場で再確認。
  同一要求内のHiddenをframeごと再照合せず、要求retire/closeで破棄。許可Pending中のholdover/thumbnail fallbackも無許可画像を出さない。cancel/兄弟mount/fork/parked後着の
  replyを他contextへ採用しない。測定で同期I/O/全結果列照合ゼロ、paint記録で無許可画像ゼロを確認。
- EのSpecifiedScaleで明示Pair/Single+blank/Split/補助を表示。A/O・gap/cap・PDF絶対需要はE oracleと一致し、
  membershipは回転/landscape後着で再pairされない。paint許可前の相方を新しいfit参照に採用しない。
  scroll/texture退去/resize/DPIでは参照unitを保持、現在のtrim/rotationで再解決、手動zoom/panは維持。
  構成reload/実削除はsource/role/slot/sliceの証明でfit参照を保持/remap/失効し、同idxの別identityを参照しない。
  mounted/parked両ownerと別scope兄弟不変を検証する。指定サイズの可視上限でG独自auto zoomなし。
- Book詳細sortは構成順lock、抽出詳細sortは結果順、通常folder詳細sortは従来。
- trim左右別/ページoverride/Blankの実slot/補助/RTL、Singleはsingle margin、既存非構成本は不変。
  Blank source/readinessなし、実Navigationだけをresume/export targetへ。

**2026-10-11 G3実装の所有境界**:

`book_composition/projection`はraw manifestの全直下参照を検証し、構成順rowsとresolved unitを同時に作る。
ZIPはpolicy投影前のraw treeと従来のdisplay prefixを保持し、PDF/EPUBは物理ページ番号とsource spellingを保持する。
未登録tailだけ中央Singleへ追加し、hidden/Missing/補助を別unitと組み直さない。
Preparing→Projecting→Acceptedを既存context ownerに載せ、workerでrows・個別編集・raw位置表を準備する。
採用直前のscope/token/items generationを照合し、準備中に動いた最新anchorもidentityで解決する。
構成更新による着地はresumeを書かず、次の通常読書操作で既存writerへ戻る。
未保存trimは既存page keyのbatchをcomposition serviceのFIFOへ受付し、workerの保存完了後にrowsを置き換える。
通常のtrim保存・resetとRemoteのtrim変更も同じFIFOへ通し、古い投影workerが新しい編集を上書きしない。
Remoteは既存要求ownerでreceiptを待ち、保存後にだけ正規化した設定値を返す。
既存Edit ledgerの書込guardは受付時から完了まで保持し、キュー待ちの間に読取stampを成立させない。
採用時にも編集stampを照合し、準備完了後の保存済み編集を古いsnapshotで置き換えない。
準備中の新しい編集は次のprojectionへ引き継ぎ、失敗時は元列とdirty編集を保持してlog/通知する。
root/key/storeは既存view_trim.dbのまま、UIでDB/FSを開かない。

表示許可はcontext/items世代・source identity・role/slot/sliceごとの有限なownerであり、texture cacheを許可証にしない。
相方・Split・補助は新occurrenceとして同じunit内でatomicに照合する。Checking/Hiddenではtexture選択、paint、
transform/hit/読了trace、新しい指定サイズfit参照を採用しない。測定extentはbounded cacheで保つが画像許可を持たない。
類似候補の長押しプレビューも既存のSingle/Spread/連結の表示経路を維持し、置換元の提示許可を共有する。
連結の完了取得をpaged専用条件で止めず、通常の可視unit受付で許可されたoccurrenceだけを置き換える。
候補そのものは既存preview request/worker/authorizationへ構成の証明を載せ、共通identity resolverで確認する。
確認は既存serviceのFIFOで初期化・maintenance・先行保存の後に行い、preview workerの私有channelで受け取る。
押下時snapshotの初期化待ちや未反映の保存を許可/拒否と解釈せず、UIで待機しない。
完了時の最新token照合で古い許可を採用せず、次の長押しで再確認する。query/thumbnailへのlookupは追加しない。
UnsupportedSourceは採択済みの通知・logと従来表示、読取失敗は通知・logで拒否し、別のpending/retry ownerを作らない。
連結読みのskipは一つの要求の有限な候補とcursorだけを持ち、退出/close/source変更で破棄する。
連続する非表示範囲はcursorへまとめ、離れた範囲は現在の画面要求に必要な最大20範囲へ限定する。
退出した範囲を捨て、再入場では再照合する。長い全非表示列もanchorを先に動かさず有限に終端へ進む。
SingleからSpreadへの切替は同じdisplay cacheのtyped phaseへ、許可済みの旧descriptorを一時保持する。
新しい相方のChecking中は旧source/role/slot/sliceだけを再表示し、新unitの確定で一回置換する。
旧descriptorの世代・source・scope tokenが変われば保持を終了し、textureだけから許可を復活させない。
保存通知で抽出結果列やthumbnailをreloadせず、次に提示する候補だけworkerで照合する。
明示targetは構成採用前にidentityを捕捉し、自動開始と区別する。初回列挙の継続はsource完了まで既存deferred ownerへ保持する。
compositionの定期pollはsource待ちの継続を消費しない。列挙・hydration完了境界でだけ、
構成待ちの継続を同contextの提示許可ownerの`AwaitingProjection`へ移す。
Composed/Automaticどちらの採用でもscopeとcontextを照合し、新世代へ一括で引き継いでからtargetを解決する。
取消・close・source置換で破棄し、forkへ未完了要求を複製しない。待機表示とnavigation lockもこのtyped要求を参照する。
これは全libで観測したRAR/ZIP/CBRのmulti-window open/navigation失敗の根本修正であり、
sourceの未完成rowsへ早期openする組合せをなくす。source pendingの有無によるguard、sleep/retryは加えない。
slideshowも同じtyped permission intentへsliceと継続を載せ、許可後の着地だけ選択・timerへ採用する。
同一ページの反対側は許可済みsliceだけを確定し、既存どおりsourceの再open・世代変更・history更新を行わない。
待機中の旧timerは新しい要求を作らず、停止はその継続を破棄する。

簡素化は既存のread/sidecar/permission ownerへ継続を載せ、未採用rowsを表示へ混ぜない形を採用した。
全列の都度照合、全体modal化、設定保存のたびに抽出窓を閉じる案は、通常閲覧と結果保持を変えるため使わない。
失敗への自動retryやrollback ownerは追加しない。
mode-onlyは既存spread.dbのmode整数に表示分類を載せ、元の結合binding値を下位部分に保持する。
binding未設定は専用値で表し、mode行作成だけでLTR/Pagedを作らない。direction/flow/端配置/補助/shiftを変更しない。
読取値は休眠bindingと表示分類を分け、exact→fallback→既定のbinding/方向/表紙を解決してから分類を適用する。
旧Single/Verticalの方向列、binding未設定時の親RtlCoverも解除・再入場・metadata復元・Remoteで共通に維持する。
文書方向追従ONでbinding/方向が未設定なら、文書由来の既定も解決してから表示分類を適用する。
Singleにも解決済み方向を保持し、Spreadは既定の表紙指定、Splitは同じ方向を使う。
構成解除・PC再入場・Remote再入場は`StoredSpreadState::resolve_reading`を共有し、
文書方向の取得要否も`needs_document_direction`で決める。表示分類だけの保存は取得を止めない。
構成解除ではaccepted sourceが元EPUBの`ReadTarget`/世代leaseを保持し、その世代の方向を使う。
PDFの方向が必要なら投影workerで既存read-target列挙を行い、所有contextのpasswordを渡す。
UIに文書/DB/FS読取や待機を追加しない。解決を一箇所に集約し、追加の復元stateは作らない。

2026-10-11 P2-3再修正の公開版調査（ソース確認、製品起動による観測ではない）:
v4.5.0 tag `1ff9d07df`には`StoredViewMode`/binding未設定codecがなく、通常のPC/Remote保存は
結合modeと方向を保存する。PC/Remoteとも保存mode・方向がなければ文書方向を採用する。
従って「binding未設定＋mode-only保存」の今回の不一致は公開版では成立せず、G3で導入した保存状態の不具合。
旧`SpreadDb::set`のmode列だけの更新も、方向/表紙を含む結合bindingを保存するため、新mode-onlyとは異なる。
公開版の根拠は`spread_db.rs`の旧読取型/NOT NULL schemaと`set_presentation_state`、
`ui_fullscreen.rs::persist_spread_presentation`、`remote_ipc/ui.rs::persist_remote_spread`、両再入場の方向採用条件。
回帰fixtureはbinding/方向未設定・全体LTR表紙あり・RTL EPUBに対するSingle/Spread/Splitで、
実際の解除・PC再入場とRemote応答のmode/方向、追従OFF、保存行不変を検査する。
別のEPUB世代が公開されても、構成解除は元のaccepted世代の方向を維持する。

解除用shift復元causeは実際のComposed→Automatic採用にだけ与える。構成なしのgeneration再照合や★固定解除は
通常の保持列復帰であり、snapshot ownerが復元した記憶OFFのsession shiftを保持する。
required Hidden初回openはMissingと同じcontext限定の既存終端処理へ渡し、Preparing sessionとloading runtimeを終了する。
兄弟contextのitems/worker/選択やmain一覧は変更しない。
専用store/column/migrationは追加せず、metadata transferもmode値を検証・保持する。
`FsViewSpread` / `FsViewLandscapeSplit`は既定キーなしの通常登録Action。
Remoteはstrict IPC **v70**で`SetViewMode`とderived `composition_managed`を追加し、Composedの結合`SetSpread`を拒否する。
Webのmode切替は維持する。E C1–C12をproduction scope/admission/restore ownerへ接続し、保存規則をGで複製しない。
G4のeditor/save modal/resume intent、G4Fのfolder sidecar、G5の構成editor/exportは次段。
検証実数・exit・ログは`target/G-1242-g3-msg.txt`に記録する。

### G4: modal editor / 保存reload / 多窓 (書庫・PDF・EPUB)

所有: editor、operation admission、context registry、snapshot/保存resume intent。
受入:

- cancel無変更、undo/redo、direction/cover/補助/端配置/単独化をdraft内だけ編集、未操作Pairの再計算なし。
- 白紙は実ページカードに従属する半透明/破線/label。独立selection/drag不可、単ページでskipする説明。
  blank-only/二枚目拒否は通知 + draft/revision不変、正常追加は§1.240 viewer appearance。
- 保存前に元窓着地先を表示。Book全更新窓/退避Bookは旧各窓順の次→前→新先頭→空。
  結果窓の現在画像/一覧は保存時不変、復帰時の新規提示だけ遅延照合/同じ着地規則。
- Preparing/Editing/Saving/Reloading全phaseでopen/history/F12/別窓/keys/settings/file操作をmodal ownerで遮断。
  Remoteは後の取得優先。editor→BeginAcquire/逆順、prepare後着、mainのみ/fullscreenなし、兄弟一時mount、
  owner close/abortを決定的handler試験で検証。送信済save成功/失敗の一回終端前にFinishAcquireしない。
- manifest変化は保存拒否 + draft保持、DB失敗は旧record + 存続draft保持、終了後のdraft復旧/自動retryなし。
  unreadable構成は原本保持/書込停止/通知、明示「今回は構成を使わず開く」は選択窓/本だけで記憶しない。
- Bookの★固定解除/元一覧復帰/F12元一覧/parked/historyは依存不一致でreload。結果保持列は構成依存を持たずgrid復帰lookupゼロ。
  membership/source label/return route/scaleは保持、read-only mount/fork/closeは兄弟無変更。
- 同一本A20→21/B80→81でreload完了順A→B/B→Aを反転。save owner=Aなら共有resumeは常に21/最大一enqueue。
  parked/退避/F12復帰は81を書かない。一覧のみ元窓はcaptured既存位置、位置なし/対象外/全非表示はKeep。
  commit失敗writeゼロ、Saving中Remote closeも同一intent、import背景更新はKeep。
  reload/退避復帰後の初回paint・寸法/指定サイズfit後着によるcontinuous自動reanchorもwriteゼロ。
  次の通常読書要求 (ユーザーナビ/再開自動送り) では既存writeを再開し、位置記憶を恒久的に止めない。
- snapshot: light/dark、1093×614/1366×728、倍率100/200%、長名、多ページ、白紙/エラーlabel、button到達、IME helper。
  HUD indicator/無効理由「本の構成で設定されています」/有効editor entryとmode切替。
- Eの前面wheel ownerを実HUD経路で検証。editor entry + wheel/閉じるkeyを同frameに送っても、
  背景metadata/nav/固定navigatorは不変。旧popupのwheel平滑化残りを次frameのmodalへ流さず、
  modal終了後の新入力は従来のownerへ戻る。IME確定/取消はhelperで、表示mode/fitの維持を確認。

#### G4実装の接点・UI判断（2026-10-11、next-book）

保存後の各窓の着地は、ComposedだけでなくAutomaticからの初回適用でも旧実ページ順を
Refresh要求に保持し、同じ次→前→新先頭のresolverへ渡す。完了順による共有位置の変更はしない。
共有resumeは既存writerのdrive-stripped keyと同値dedupを使い、seedが元manifestにない時はKeep。
「今回は構成を使わず開く」は選択contextで元列をworker準備し、ページ編集も同じ採用で戻す。
通常Automaticの早期採用とは分け、Ignoredの選択を保存データや兄弟窓へ公開しない。
Ignoreを含む列置換の判定は保存票の発行・worker・採用で共通化する。未保存の余白は
既存FIFOへ先に提出し、その保存完了後に同じ実ページキーの編集を読み、元列を採用する。

実装は `app/book_composition_editor` のprocess共通owner/ticket受付と、
context bundleが保持する `CompositionEditorState` に分ける。Preparing→Editing→Saving→Reloadingを
単一phaseとして扱い、兄弟の一時mountはownerを変更しない。大きなprepare入力・draft/sessionは
context所有のBoxに置き、ClosedのAppやbundleへその全サイズを埋め込まない。
開始/保存は既存session lifecycle mutexの
短いLocal admissionで採用し、workerの列挙・fingerprint・構成writer待ちをそのlock内へ置かない。
送信済み保存ticketはeditor/window closeで破棄せず、共通完了ownerが一回回収してRemote取得barrierへ渡す。
各窓のreload着地と、編集元の捕捉したresume intentの一回受付は別の責任として扱う。

`book_composition/editor` のdraftは明示unitを保ち、操作previewを経て方向/表紙/Pair/Single/空き側を
変更する。既存Pairを壊す操作、方向・表紙で組を作り直す操作は変更される組を確認してから適用する。
確認要否は操作名ではなく変更前と候補の最終Pairを比較する。B/Cの組み替え等の複数操作も
候補だけを用意し、取消ではdraft/historyを変更せず、確定後は一回のundoで元へ戻す。
「編集中の条件で組を作り直す」はdraftのページ順/方向/表紙で隣接Pairを作り直す。
全体既定を書き換えず、横長判定・末尾・単独・ずらしなどの自動規則を再適用しない。
確認中は別編集を止める簡素化を採用し、複数previewの同時進行・自動merge・割込み復帰を作らない。
元のmanifestが変わった場合はdraftを保持して保存を拒否し、明示的な読み直しだけで新draftを用意する。

`ui_composition_editor` はview/actionのみを受けるI/Oなしの共通描画。
幅に余裕があれば元ページとunitを二列、狭い画面では同じ二一覧をタブで表示する。
元ページ・unit・変更される組の確認一覧はvirtualized、可視thumbnail要求は一frame最大24ページ。
thumbnail worker/texture/channel/cancelはeditorの寿命だけが所有し、背景viewerのqueueやcacheを流用しない。
policy除外は名前と理由だけを表示してdecodeせず、Missingは参照を保持して表示する。
Windowはdefault_posを使って移動/resize可能とし、本文だけをscrollして保存/キャンセル/解除を下端へ残す。
通常文字・補足・errorは共通themeの色を使い、白紙は独立した画像/選択/drag対象にしない。
Enter/EscapeはAppのIME helperで得た値だけを渡し、raw key eventから保存/取消を決めない。
Escapeは解除/作り直しの確認、変更previewの順に先に閉じる。previewの取消は候補だけを
破棄し、それまでのdraftとundo履歴を保持する。確認がない時だけ編集全体をキャンセルする。
共通ownerがある間は背面のmenu・toolbar・検索等を無効にし、foregroundのmodal layerへ入力を限定する。
兄弟の一時mountでも同じownerを参照し、そこで別のeditorを生成しない。
slideshow・連結の自動reanchor・一覧watcherの反映も既存処理の入口で保留し、
再生設定やzoom/pan/flowは保持する。modal終了後は通常の入力・更新経路へ戻す。

入口は本HUDの既存overflow、本一覧右クリック、main「製本」menu。
右クリックは `context_menu_nodes` の一つの定義をnative/egui fallbackへ渡し、別メニュー定義を作らない。
main/一覧背景の入口は選択行やページ数を資格にしないため空/全非表示本でも残る。
HUD root surfaceは同じpopup layerでキーより前に描画/受付し、editorを採用したframeの旧Escapeは背景へ渡さない。
前面popupは描画順だけを変えるchild Uiではなく、実際のpopup範囲だけのAreaとしてhit-testへ登録する。
内側のwheelはそのScrollAreaへ、外側のwheelは従来の背面ownerへ配送する。
Eのframe開始時wheel ownerを保持し、旧overflowのwheelと平滑化残りを終端してからeditorを描く。
editorへ入らない通常overflowのキー終了・既存操作は維持する。追加アプリshortcutは設けない。

純粋UI/snapshot定義は `tests/book_composition_editor_snapshots.rs`、実HUDの同frame入力と
通常overflowの回帰は `ui_fullscreen`、C7の実保存/取消/失敗・ownerの試験はApp editor側が所有する。
PNG生成・目視、最終共有gate・確認build・独立レビューの実行証拠は検証ownerが別途記録する。
この実装記述をテスト成功や実機確認の証拠とは扱わない。出荷は利用者決定どおり**次々回release**、
今回のmasterには統合しない。画像folder/sidecarはG4F、Remote/出力の残る契約はG5を維持する。

### G4F: 画像だけの本フォルダ (独立・切り離し可能)

所有: folder資格/adoption、RelativePath PageRef migration、sidecar全snapshot/構成family/消費receipt、folder用editor入口。
G1〜G4の書庫/PDF/EPUB契約を変更せず、共通pure model/storeへfolder adapterだけを追加する。
受入:

- image-only Book作成/並べ替え/非表示/元一覧/ページ一覧/同folder兄弟窓。OFFはOrdinaryList/既存sort、
  mixed動画/音声は従来閲覧、再適格なら構成再適用。子containerだけの場合は現行本分類を尊重。
- 個別file改名の相対ref更新、scope外移動はMissing、root改名/copy、folder削除/page削除の差。
- backup ON/defaultはroot構成ミラー、OFFはDBのみ/自動restoreなし。
  タグだけONのwriteでは既存rootを保持し、DB構成値への更新/自動restoreをしない。
  編集/タグitemsを保持、ページ保存/dirty flush/終了でroot構成を失わず、sidecar二重writerなし。
- 新場所/別driveにfolder+sidecarをコピー、DB構成なしで**自動**不足行復元、確認promptゼロ。
  既存DB構成/破損行は上書きしない。旧場所DB行保持。sidecarなし/OFFは明示bundle手段。
- v1既存編集/タグを保持してv2往復、未知/破損sidecar原本保護、Unsafe refs/blank-only拒否。
  markerのmtime秒値を構成へ流用しない。edit同期済みでも構成未適用なら構成familyだけを復元する。
- **構成だけのsidecar** を書込/読込/PendingWriter経由で復元できる。最後の個別編集を削除してもroot保持、
  items/root共に空の時だけfile削除。ページ/タグのreplaceでrootが消えない。
- flushを予約→items不変のままrootを変更→旧flush完了。新rootのdirty/新ownerをclear/evictしない。
  rootだけのPendingWriter差替えは旧sequence/Arcのrevalidateを拒否。root変更/消去、requested family差、
  writer pending/failed、同len/mtime別bytesはretained reuseを拒否してstrict probeへ戻る。
  PendingWriterに残る消費済みrootも復活させず、process sequenceをdisk markerへ保存しない。
  prepare後に解除/別構成保存が完了してからrestoreがcommitする順でも、transaction内の再判定で上書きしない。
- **明示metadata importの新形式空反映**: 複数folderの構成を一括解除し、消費receiptを各項目transactionで更新。
  一回のsummary確認だけ。古いbackupが残っても再入場/再起動で復活せず、旧形式/coverage外/skipは構成保持。
  ページ編集/タグfamilyの同期・復元は構成receiptにより抑えられない。
- **解除後に旧backupが残る場合**: OFFで解除→ON→再入場/再起動で同tokenを消費済みと判定し構成なしを維持。
  OFF→解除→タグbackup更新→ON→再入場では全file tokenが変わってもroot witness一致で再挿入しない。
  root自体の変更 (同len/mtimeも含む) は別sourceと判定。別場所へコピーしたfolderは不足行を通常復元する。
- backup writeだけ失敗は成功DB/解除receipt保持 + 通知、旧backupの同場所再復元なし。
  source準備/DB commit前の失敗は旧DB構成、backup失敗後の自動retry/DB-file rollbackなし。
  Missing/未知/破損sourceの原本保護、receiptのrename照合/purge/reset/非転送も検証。
- 構成保存後の全結果thumbnail/一覧は不変、遅延target拒否だけ。folder-only lookupを抽出全leafへ拡げない。
- folder entryのmetadata搬送/新形式空/部分coverage、内容IDFolder ledgerなし、backup失敗は採択した通知契約。

**切り離しの判断点 (2026-10-10利用者方針)**:
G4Fのbounded設計/実装レビューで、同じ画像folder ownership/分類/復元のP1/P2が**二回連続REVISE**して
修正しても閉じない時、またはG4Fだけのために書庫/PDF/EPUBの採択済projection/editorへ新しい例外状態を
増やさなければならないと判明した時、coordinatorが作業を止め利用者へ縮小を相談する。
二回は本書の運用上の相談閾値で、欠陥を二回まで放置する許可ではない。構造問題が早く分かればその時点で相談。
相談は「画像folder対応を今期外し、ZIP等の書庫/PDF/EPUBを完成。画像folderはZIP圧縮で代替」を推奨候補とし、
費用/残る不便を示す。切る最終決定は利用者、agentが自動で対象を削らない。
採択時はfolder adapter/editor/sidecar新字段とその専用testsを外せばよく、A2/Q16/書庫等の共通型/ownerを再設計しない。
既存folder閲覧/個別編集/既存sidecar/metadata transfer機能は撤去しない。未リリースfolder構成はmigration不要。

#### G4F 実装記録（2026-10-12、next-book）

`app/book_composition_folder`を画像folder専用adapterとし、既存physical分類/列挙/重複除外を
再利用する。BookImageFolderだけが構成projection/editorへ参加し、画像本設定OFFと混在folderは
従来列を保つ。OFFでも明示folder openの不足行restoreは行い、再適格化で保存構成を適用する。
compiled folderと抽出結果の全leaf列挙へrestoreを拡げない。相対page refの個別rename、rootの
rename/purgeは共有BookScopeKeyと既存maintenanceへ接続し、Folder内容ID ledgerは追加しない。

sidecar v2は`folder_composition`を任意root値として持ち、v1 readerは維持する。rootとitemsは
一つのimmutable SidecarSnapshotであり、各ArcをCOWで共有する。既存writerのFIFOとfull-snapshot
identityが予約/完了/失効を所有し、構成専用writerやroot dirty flagを足さない。
構成保存はG4のmodal ownerとsave ticketを再利用し、同folderの既存sidecar flushをworkerで
完了させてからDB保存/ミラーを行う。終了時も同じwriterを使う。タグだけのwriteは既存rootを
保持し、構成DB値を取得して差し替えない。UIはworkerが公開したroot Arcを採用するだけで、
sidecar read/write/hashや大きなrootのcloneを行わない。
保存票があるfolderの通常/定期flushと、別folderを開くChecking workerのflush予約は、
その票の完了採用まで同ownerへ委ねる。所有viewerを閉じてもApp共通の送信済票は残り、
旧snapshotをCheckingへ予約しない。予約後のArc照合だけでdisk上書きを防ごうとしない。
個別page改名は既存migration ownerがworkerへ渡したbackup設定を保持し、同folderの
書込権をミラーからUIキャッシュ採用まで持つ。workerのmaintenance解除によるroot Arc公開を
完了応答が証明し、応答の消費内でdirty itemsを維持したままrootを採用してから書込権を返す。
通常service pollの順序へ採用を委ねない。backup OFFの改名ではタグだけのbackup rootを維持する。
改名ミラーはtransactionが割り当てた現revisionを含む保存値を使用し、DB/ミラー/公開Arcを一致させる。
遅いtag完了はcacheのitemsを変更できるが、workerの新rootを旧snapshotで上書きしない。
採用後はdirty itemsを維持して新rootと共に既存writerへ送る。他folderのflushは続ける。
列切替でキャッシュを退役する入口も書込ownerのsnapshotを保持し、既存root-close barrierは
構成ミラー中のmigrationを同じ完了境界まで待つ。構成を持たない従来改名はこの終了待ちの対象外。

同型の製本page並べ替え/移動も既存migration queueの型付き段階
`BookComposition → Collection`へ載せる。既存の個別改名は`Generic → Collection`を維持する。
構成移行を直接fire-and-forgetしてからcollection-onlyへ進めない。backup設定は受付時に捕捉し、
queued/in-flightとも同じ書込権を持つ。既存journalのGeneric/Collection JSONは変更せず、
新しい構成段階だけoptionalなbackup設定を記録する。既存journalの変換/再作成は行わない。
runtimeの段階は排他的enumであり、二つのpending flagへ分散しない。

G4F REVISEの完了順fixtureは「保存受付→所有detached contextを閉じる→実workerのミラー完了→
別folderのChecking」と「dirty tags→個別page改名/製本page移動の実worker完了→採用前flush→応答採用」を固定する。
経過時間のsleep/retryで順序を作らず、既存worker/channelの完了を待つ。書込claim専用ownerや
root dirty flagは追加しない。全体openを新たにmodal化するより、既存save ticket/migration ownerの
書込権を全予約入口へ揃える方が、他folderの閲覧/flushを維持したまま状態の組合せを減らせる。

自動restoreは既存sidecar import ownerの独立composition familyへ載せる。strict tokenの
SHA-256/metadata照合とfull-snapshot PendingWriter照合を再利用し、root witnessと実disk markerを
構成DBの消費receiptへ保存する。receiptは不足行restore/明示解除と同じtransactionで更新し、
commit時に現行行を再確認する。receiptは転送しない。root renameは移動先sidecar照合後、
purge/resetはreceipt-only行も扱う。表示開始は同contextのsource restore終端後に既存projectionへ
進み、Resumingが依存するBook準備を止めない。兄弟窓のrestore ownerや行は変更しない。
画像Bookの自動fullscreenは既存の本用deferred-open ownerへ渡し、構成採用前の元indexを
明示page identityとして固定しない。「最初から」は採用した構成の先頭、「続きから」は
同じresume/target resolverで解決する。画像Book以外のfolderは従来入口を維持する。

明示metadata importは既存の一回summary確認と項目SAVEPOINTを使い、新形式空もroot消費を
記録する。backup OFFではDB/receiptのみ、ONではcommit後に既存writerへミラーする。
backupだけ失敗した場合はDB成功/receiptを保持し、
「フォルダのバックアップを更新できませんでした」と通知する。古いbackupが残るため、
移動前に保存し直すか明示メタ情報exportを使う案内を付ける。自動retry/DB-file rollbackはしない。

検討した簡素化は、folder専用restore/editor待機を追加せず既存modal/FIFO/import ownerを使うこと。
その構造を採用した。切り離し時はfolder adapter、folder source分岐、sidecar root/receipt/familyと
専用fixtureを除けばよく、共通model/unit/提示許可、書庫/PDF/EPUBのsource ownerは維持できる。
Remote wireは変更していない。G5と実機確認は別工程であり、本節は受入/レビュー成功の主張ではない。
自動検証の実数と失敗は`target/G-1242-g4f-msg.txt`へ記録する。

### G5: Remote / 出力 / 最終gateと文書

所有: ContainerEngine/IPC/Web、export/製本入口、docs/manual。
G4F採択時だけfolder専用fixtureを追加し、切った場合はarchives/PDF/EPUBで同じ共通gateを完了できる。
受入:

- PC/Remote Book entries/PageGroup/anchor/placement parity、budget途中Pair、hidden refusal、端末single強制。
- ExtractedResultは初期Single/一時Spread/DB保存なし。通常thumbnailの維持、候補だけhidden照合、Web独自pairingなし。
- SetSpread結合要求/SetFinalCover/SetSingleton/SetPageAloneは構成ありで理由付き拒否、同値/再送/直接IPCもwriteゼロ。
  Webは同じ無効説明。mode-onlyのSingle/Spread/Splitはaccepted構成へ戻れ、bindingを書かない。
- strict protocol/asset token一致、Remote取得close/save barrier/acquire cache refresh、必要な従来remote generation失効。
- 選択export/全冊追加/外部渡しのorder/hidden除外、結果selectionは選択対象だけ照合、white/supplement sourceなし。
  製本出力へ構成/Blank/Pairをコピーせず、元書庫を変更しない。
- source/cache移動/再変換/content restore/metadata import結果一致。複数本の追加/置換/解除は一括確認一回。
- manualにEPUB再変換の番号ずれ許容、A2のreleased変更、遅延hidden (サムネは残る)、初期化/解除、
  folder scope採否とbackup設定/sidecar v2互換/ZIP代替、失敗時の再操作を明記。

各stage実装者が狭いunit/handler/integration/snapshot検証を所有し、独立reviewerは有効な結果を再利用する。
最終共有gate: scripts/test-full.ps1、fmt check、UI文字変更glyph checker 0件、scripts/build-dev.ps1で利用者用binary。
設計だけの今回はbuild/test/app launchは不要・未実施。interactive確認は別の事前承認suite、normal profile起動は利用者のみ。
将来の確認は書庫/PDF/EPUB (G4F採択時folder)、F12両窓、Remote、白紙/trim、A2/遅延拒否、E指定scaleを列挙する。

実装時更新: architecture/virtual-folders/display-pipeline/web-remote/keymap、metadata transfer、EPUB仕様、
manual本/見開き/検索/設定/Remote、保存先説明、docs索引、detached §11。設計turnは本書とhandoffだけを変更する。

### G0照合記録: E統合後の接点と変更一覧 (76488f0ec)

**変更一覧** (元の設計/参照から変えた点を一行ずつ記録):

| # | 再照合結果・今回の更新 | 影響する契約 / 実装stage |
| --- | --- | --- |
| 1 | §2の旧185引用を全て同treeへ更新。punctuation/空行を根拠にした参照は該当function/判定へ置換。本文の旧source引用も更新 | 列生成/下流/同期の前提は維持。実行成功の証拠ではない |
| 2 | Eのunit/anchor resolverは既存正本を維持、fitはtyped A/Oと指定値/capへ拡張。新しい前提20を追加 | Gはorder/role/slotだけを供給し、Eのfit式を複製しない。G3 |
| 3 | 連結の参照unitと保持sourceを見落とさない条件、source置換時の失効を具体化 | `fs_continuous_fit_reference` はbundle所有。idx一致だけで旧geometryを新Bookへ再利用しない。G1/G3/G4 |
| 4 | HUD前面wheel配送/キー終了frame/ナビゲータ入力射影を構成editor入口にも接続 | modalの開始と既存popupの入力終端を分離。Eのframe-local ownerを再利用。G4 |
| 5 | session.innerとRemote UI queue admissionのmutexを明確に区別、start/saveのlock順とsend失敗の受付終端を明記 | §8.1。同じinnerでLocal確認とowner/ticket採用、workerは逆lockなし。G4 |
| 6 | 現行close/AI barrierにまだないeditor/ticketのFinish条件を明記 | §10.2。送信済save ticketをdraftと共に落とさない。既存finish_acquire/30秒watchdogを維持。G4/G5 |
| 7 | 退避列の直接代入はE後も残る。共通return adoptionが必要な点と、E状態の維持/失効条件を追記 | §10.1。Book依存不一致はreload、結果gridはlookupなし。G1/G4 |
| 8 | 通常open/seek/continuous reanchorのresume writeを再確認し、adoption causeを記録入口まで渡す | §6.1。各窓の着地と一つのresume intentを分離。G3/G4 |
| 9 | Q16可視候補受付にEの指定サイズ参照選択/geometry入力の許可境界も含める | §6.2。cached Hidden/new partnerのpaintだけでなく新fit基準への採用も防ぐ。G3 |
| 10 | A2 modeとEのfit settingを別軸として確認。既存結合spread setterは依然分離が必要 | §9.2。A2はSingle初期値/一時modeだけ、Eの5方式/手動zoomは維持。G1/G3 |
| 11 | sidecar token/writer/予約/reuse/receiptのソースはEで変更なし。receipt所有のP3訂正を維持 | §7.4。receiptはApp共通storeのみ、F12はprojection/permit。G4F |
| 12 | §1.364は別ライン設計中。必要な8契約を§9.3へ集約しG0の保留を明示 | Gはshift store/schema/fallbackを追加しない。E採択後にG3共同fixtureを確定 |
| 13 | 現行shift handlerがSingle/Splitの一ページnavも担当することを確認。shift拒否前に通常navへ分離する順序を明記 | §9。既に維持を決定した単ページ移動を入口guardで失わない。G3/E契約 |

**Eのgeometry/scale/unit/anchorへの具体的な接続**:

- canonical unit/Navigation/presentationは `SpreadDisplayComposition` → `fs_navigation_display_demand_for_anchor`
  (`src/ui_fullscreen.rs:17103`, `:17146`) を使用。accepted anchorと画面左右のfirst/secondを混同しない。
  明示Pairは横長/回転後着でも再pairせず、Gの確定unitをEのlayout resolverへ渡す。
- `FullscreenFitInput` / `FullscreenFitBasis` / `resolve_specified_scale`
  (`src/fullscreen_specified_scale.rs:52`, `:13`, `:114`) はA/O、指定値、最大倍率、no_up/downを解く。
  blankをsource basisへ追加せず、実ページのSingle placement/仮想slotを既存geometryへ渡す。
  Pairは二枚 + 実効gapをfitし、既定gapを保存し直さない。width/heightの指定値を構成recordへコピーしない。
- Eのcapture/holdoverはfit input/limits/元寸法からlayout枠とsampling枠を分ける
  (`src/app.rs:11267`, `src/displayed_image_transform.rs:288`, `src/ui_fullscreen.rs:10511`, `:41571`)。
  旧描画をlive指定値/新raster/現在の構成から再解決しない。source identity付きPDF絶対需要は
  `PdfRerenderDemand` (`src/app.rs:7902`) を維持し、G独自のpartner用需要ownerを作らない。
- 連結の `ContinuousSpecifiedFitReference` / retained source (`src/ui_fullscreen.rs:8163`, `:8177`) は
  `ViewerContextBundle.fs_continuous_fit_reference` (`src/app/viewer_context_registry.rs:1052`, swap `:2691`) が所有。
  scrollで参照unitを選び直さず、texture/cache退去後も元寸法を保持し、現在のtrim/rotationでgeometryを解く
  (`src/ui_fullscreen.rs:38245`, `:38340`)。Q8はtrim sideの解決だけを替え、回転/分割時の既存trim例外を統一しない。
- items置換は既存 `invalidate_idx_state_and_queues` による参照/PDF需要失効
  (`src/app.rs:42075`, `:42099`) を使う。純slot remapで同じidentity/role/slot/sliceが残る場合だけEの
  `ContinuousSpecifiedFitReference::remap_indices` (`src/ui_fullscreen.rs:8192`) で保持できる。
  parked bundleの `set_items_generation` (`src/app/viewer_context_registry.rs:1813`) は現状layoutを消すだけなので、
  新投影のadoption ownerがsource/role/slot/slice変更の参照失効も担当する。現在mountのAppだけへ失効を送らない。
  anchor/相方削除、構成の順/Pair/配置変更で前提が壊れる場合はそのcontextの参照を失効し、新しい許可済みunitを選ぶ。
  §3.2のitems/unit/identity remapに含め、F5を待って直す設計にはしない。
- 結果列では構成保存通知によるfit参照全resetを追加しない。既に採用した画面外の参照sourceはmeasurementに使えるが、
  新規提示permitを兼ねない。新Pair/相方のChecking中は新fit参照を確定せず、旧許可済み表示/背景を維持する。
  resize/DPI/共通cap変更で同じ参照unitのfitを再解決し、手動zoom/panを巻き戻さない。
  指定サイズの可視上限は既存遠方unit除外 (`src/ui_fullscreen.rs:38628`, `:38646`) に従い、Gでauto zoomを足さない。

**HUD popupの入力所有**:

`render_fullscreen_viewport` はキー/背面consumerより前に前面wheel ownerを捕捉
(`src/ui_fullscreen.rs:23779`)。`fullscreen_front_popup_wheel_target` (`:31700`) と
`FsHudPopupWheelInput` (`:44217`) がTopBar/SeekStrip/Overflowへ配送し、閉じるframeにも残りを消費して
`discard_pending_scroll`する (`:25295`, `:25302`)。navigatorも同じconsumer射影を使う (`:33214`)。
構成indicator/無効理由/editor entryは既存HUD popup描画内に置き、別Areaの入力ownerを作らない。
entryを押したframeでpopupを閉じてeditorを開始しても、既存のframe開始時ownerが残りのwheelを終端し、
背景nav/metadata/ナビゲータへ流さない。modal開始後はmount非依存editor ownerがkeys/gesture/openを受付拒否する。
前面のpopup制御と本の構成action admissionを一つのboolへ混ぜず、既存IME helpersを使う。

**G0構造点の照合状態**:

| 点 | 現行コードの確認 | 採択済み設計への接続 / 残るgate |
| --- | --- | --- |
| editor start/save | `SessionStateMachine::acquire` (`src/remote_ipc/session.rs:309`) が先にphase確定。innerはprivate | §8.1のsession helperを追加。実装/handler試験と次レビュー |
| close barrier | `src/remote_ipc/ui.rs:1081` は全窓close + AI静止、wait_until_activeは取得中待機 | §10.2のticket終端を既存barrierへ追加。新拒否gateなし |
| Book return adoption | snapshot直接復帰 `src/app/snapshot_ops.rs:1282`, `:1406` / F12 `src/app/viewer_context_registry.rs:3418` は残る | §10.1共通adoption。E追加runtimeのscope/世代も保持/失効 |
| single resume intent | path一行/優先順位なしは変わらず、continuous reanchorもwrite | §6.1の一回intentとtyped adoption cause。A/B完了順反転試験 |
| Q16/continuous permit | nav early-return `src/ui_fullscreen.rs:11420`、可視列 `:38621`、texture選択 `:39285` / paint `:39395` | §6.2のcontext display demandに許可。Eのfit参照選択も許可済み新unitから |
| A2 mode boundary | `SpreadMode` (`src/settings.rs:3082`) と `apply_fullscreen_spread_mode` (`src/ui_fullscreen.rs:37798`) は結合値のまま | §9.2のmode-only routing/保存なし。Eのfit設定は別の既存owner |
| G4F snapshot/receipt | `src/sidecar.rs:463`, `:1055`, `:1148`, `:1314` / token `:203` と `src/sidecar_import.rs:795` は不変 | §7.4の全snapshot/独立family。App-wide receipt。G4Fを共通Bookに混ぜない |

G1〜G5は上記接点をbriefへ反映し、Eの既存testを弱めず回帰gateへ再利用する。
代表source fixture: `specified_scale_continuous_real_layout_preserves_reference_and_manual_zoom`
(`src/ui_fullscreen.rs:78702`)、evicted reference再解決 (`:79691`)、popup背面wheel抑止 (`:80460`)、
終了frame (`:80705`)、固定navigator (`:80863`)。今回はこれらを**読んだだけで再実行していない**。
detached §11の追記案は `target/G-1242-g0-msg.txt`。次レビュー/設計担当合意後にcoordinatorが反映し、
本turnで `detached-rework-plan.md` は変更しない。

### R2〜R5レビュー契約のR6への接続

| 既存指摘 | R6で維持/置換する契約 | stage |
| --- | --- | --- |
| editor/Remote取得・兄弟mount | mount非依存owner、同じsession start/save受付、既存全窓close/save barrier。Q9採択 | G0/G4/G5 |
| in-place items変更 | 同時slot remap/identity再解決、Missing保持、削除直後F5前のunit/resume/出力 | G1/G2/G3 |
| aggregated/mixed | 三つのrecipe。一列一layout、抽出A2、通常mixedはOrdinaryList。released変更は承認済み | G1/G3/G4F |
| 退避/兄弟hidden | Book依存の復帰adoption/各窓着地。結果の構成依存/保存時refreshはQ16で撤去 | G3/G4 |
| 転送origin/空 | composition専用logical family、Q15一括置換/空解除/旧不在保持/coverage | G2/G5 |
| Folder復元/保存時Edit | Folder ledger非対応を維持。Q10でG4F sidecar補充へ変更。EPUBもcaptured root Edit観測 | G2/G4F |
| 外部binding/周辺操作 | Q2/11/17の拒否 + mode-only維持。小編集再導入なし。抽出はDB保存なし | G3/G5 |
| 共有resume | save owner/seed捕捉、一回write、各窓anchor分離。A/B完了順反転試験を維持 | G4 |
| 音声/本を開く案内/fixture順 | mixed音声保持、案内はBook可否解決後のみ、G1→G2→G3の依存順 | G1/G3/G4F |
| R5 P2-1 旧sidecar消費 | 自動復元の実disk token、独立構成familyのreceipt。タグ更新はroot witnessで同じ消費済みbackupと判定 | G0/G4F |
| R5 P2-2 連結の新入場 | continuous可視候補ownerが許可要求を作り、texture選択前/paintまで同じ許可済みprojection。cached Hidden/新相方の試験 | G0/G3/G5 |
| R5 P3 root snapshot | 全snapshot ArcをPendingWriter/flush予約/reuseへ。構成だけ/最後の編集消去/旧flush後着を検証 | G0/G4F |

## 14. 利用者の決定 (番号固定)

全回答は2026-10-10。Q1/2/11/17はR4から維持、その他はR5で反映。backupだけの失敗はR6で採択を反映。
Q14の以前の相談はA2で不要になった。質問の番号を振り直さず、追加のscope判断も別行に記録する。

| 質問 | 決定 | 日付 |
| --- | --- | --- |
| Q1 保存済み一ページずらしを残すか | 休眠保持。現表示を初期構成へ取り込み、解除時は§1.364の現時点の記憶/identity規則で再利用 | 2026-10-10 |
| Q2 表紙・方向の変更入口 | 構成editorだけ。方向は本の作り。HUD/キー/Remote無効 + 理由/indicator/editor導線。全体/既定へ書戻さず、構成にも適用しない。view-mode切替は維持 | 2026-10-10 |
| Q3 新ページ・補助表紙 | 新ページは末尾中央Single、既存Pair/補助は不変。組替えはeditor。途中挿入が難しければ解除→再作成で現自動規則を初期化 | 2026-10-10 |
| Q4 相方のない白紙 | 保存不可、操作時に明確なエラー。白紙は実ページに付く半透明/破線の空き側表示、独立選択/移動不可。単ページではskip、viewerは既存1.240外観 | 2026-10-10 |
| Q5 保存後の着地と次回再開位置 | 各窓の旧順で次→前→新先頭→空一覧。兄弟/退避も同じ、保存前に元窓の移動先表示。resumeは編集元窓から一回だけ。結果列の保存時refreshはQ16で行わず、復帰/新提示時に遅延着地 | 2026-10-10 |
| Q6 EPUB・編集端末 | EPUBを今回含める。元EPUB+ページ番号は既存個別編集と同じ。再変換による番号内容ずれは許容しmanual明記。editorはPCだけ | 2026-10-10 |
| Q7 まれな失敗 | DB保存失敗は旧構成 + 存続draft保持/通知。保存後reload失敗は保存データ保持/再読込・再起動案内。読めない構成は保持/書込停止/通知、明示的にその窓だけ今回無視して開ける。自動retry/終了後draft復元なし | 2026-10-10 |
| Q8 白紙相方のトリム | Spreadは実ページの画面側、Singleはsingle margin。page overrideの既存優先、非構成本は不変 | 2026-10-10 |
| Q9 editor中のRemote取得 | 後のRemote優先。全PC窓/editor close、未保存draft破棄通知。送信済saveは一回完了しRemoteへ結果反映 | 2026-10-10 |
| Q10 画像folderの外部移動 | 同folderのmimageviewer.datへ構成をbackup設定 (既定ON) に従い保存。file名参照、DB不足行は既存同様に自動補充。旧場所DB保持。書庫/PDF/EPUB構成はsidecarなし/内容ID、明示bundleも維持 | 2026-10-10 |
| Q11 Remoteの方向・表紙 | Q2のlock。構成がある本のpairing関連変更は無効 + 同じ説明。小編集なし、view-mode切替維持 | 2026-10-10 |
| Q12 本扱いOFF | 実効分類OFFなら通常の順/自動閲覧へ戻し構成保持。hiddenは新提示時拒否。本扱い復帰で再適用、明示本置き場は自動OFFで解除しない | 2026-10-10 |
| Q13 mixed動画・音声 | 通常mixed順/自動閲覧とメディアを維持、保存構成は残す。Book再適格で再適用。hiddenは遅延拒否、子container行は現行資格に従い分離 | 2026-10-10 |
| Q14 抽出一覧のshift範囲 | 以前の問はA2に置換し不要。抽出列は初期Single/一時Spreadだけ、構成/本別設定/cover/shift/末尾/端/単独なし。synthetic mode keyを読み書きせず次openはSingle。released変更を承認 | 2026-10-10 |
| Q15 構成なしのメタ情報import | 新形式構成ありは置換、構成なしは解除、旧形式/対象外/skipは保持。追加/置換/解除数と説明を一括確認一回へ追加。本別promptなし | 2026-10-10 |
| Q16 結果順とhiddenチェック | 結果内直接閲覧を維持しA2適用。通常thumb表示、列生成lookup/保存refreshなし。open/次提示候補 (連結のviewport新入場・mode切替の新相方も含む) だけhiddenを調べtoast拒否/skip。しおり/評価/履歴のページtargetも同じ | 2026-10-10 |
| Q17 末尾・端配置・単独・ずらし | Q2のlock。editorの明示unit編集だけ。初期化は現在の末尾/端/単独/shift後の組を取り込む | 2026-10-10 |
| 画像folder対応の縮小候補 | G4Fで分離。二回連続の同領域P1/P2 REVISE、または共通部分へ例外状態が必要ならcoordinatorが利用者に切り離しを相談。採択時は書庫/PDF/EPUBだけ、画像folderはZIP圧縮で代替 | 2026-10-10 |
| フォルダbackupだけの保存失敗 | 成功DB保存/解除を維持して通知。古いbackup/移動前の再保存またはメタ情報書き出しを案内。自動retry/DB-file同時rollbackなし。同じ場所の旧backup消費は§7.4.2で別途保証 | 2026-10-10 |
| 構成で表せない既存ZIPのページ名 | 既存閲覧を維持。構成準備だけ通知＋ログで拒否し、原本・保存済み構成を保持して構成なしで従来どおり開く。不正参照や正規化後の重複を除去・統合しない | 2026-10-11 |
| §1.242の出荷時期 | 次々回release。next-bookで開発を継続し、今回masterへ統合しない | 2026-10-11 |

新しい利用者質問はない。R6の提示許可とsidecar同期receiptは採択済みのQ10/Q16を成立させる所有契約であり、
連結読みの制限やbackup設定の追加を提案していない。R6のACCEPTは維持し、統合後G0差分には次レビュー/設計担当確認が必要。
