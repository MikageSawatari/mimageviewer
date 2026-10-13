# §1.242 本の構成 — 設計案

更新: 2026-10-14、B2実装・検証。`next-book`、採択設計 `4f3d160f3`。
B1は独立設計レビューACCEPT後にコミット済み。B2は採択設計の実装と自動検証で、コミット・アプリ起動は行わない。
実装・検証の記録は§13のB2に記載する。実機の受入確認は別途行う。
§1.242と`book_composition.db`は未公開で、v4.6.0には含まれない。次の版に含める（2026-10-13利用者決定）。
2026-10-11の「次々回」は履歴。旧保存unitsの移行処理は不要。

現行契約は§1・§2.1/§2.2・§3〜§12・§13のB段階・§14の追記（Q25/Q26の設計担当決定を含む）。
§2の従来調査表、§7.4、§13のG0〜G5・G0照合記録は旧方式の調査・実装・検証履歴として保持する。
その固定Pair/Single、組変更確認、組ごとの手動補助指定、初期組の凍結、全pairing拒否はBの実装指示ではない。
旧R6 ACCEPTは旧方式の設計に対するもの。B1のACCEPTとB2の実装検証・実機確認は区別する。
設計履歴: R6は独立再レビューACCEPT、P3文言修正は `330e070f6`。G0照合treeは `76488f0ec`。
Eの共有BookScopeKeyと§1.364はmerge `97c9b711d`で取り込み済み。G3の実装・検証結果は§13とhandoffへ記録し、受入とは区別する。

設計節の型名・ファイル名には提案時の表記も残る。実装済み段階の所有境界は§13を参照する。
§2とG0の現状記録は当時のソース調査であり、製品を起動して観測した記録ではない。

**2026-10-11の開発・出荷判断**: G2 part B (`249dc75a0` + `77ca29b29`) は独立レビューでACCEPT。
§1.242は次々回releaseへ送り、`next-book`でG3以降を開発する。今回masterへ統合しない。

## 1. 要件と範囲

旧要件の入口は [next-release-backlog.md §1.242 / §1.364](next-release-backlog.md)。
Bの正本は本書の2026-10-13改訂。2026-09-15/10-10の決定履歴と区別する。

- ZIP/CBZ、PDF、EPUB、直接閲覧・変換閲覧する対応書庫に、本単位の非破壊な構成を保存する。
  書庫階層ごとに独立する。製本フォルダは構成作成対象外。変換書庫の保存先 identity は元書庫。
- 保存する利用者指定は読む方向、表紙あり/なし、ページ順序、非表示、ページごとの表示指定だけ。
  組の一覧は保存せず、順序・非表示を適用した実ページ列を通常の見開きbuilderへ渡して毎回自動生成する。
  離れたページのPairと組単位移動は廃止。組にしたいページは左の元ページ一覧で隣へ並べる。
- 非表示は左だけで指定・確認。右の結果・Book一覧・閲覧列へ出さず、残りを繰り上げて組み直す。
  原本、個別編集、しおり等は消さない。非表示と実ファイル削除を混同しない。
- 指定なしは通常と同じ横長・表紙・端配置・強制単独規則。環境設定と既存本別preferenceを尊重する。
  direction/coverだけは構成recordの値を優先。表紙は先頭の可視ページ（任意PageRefの表紙指定は廃止）。
- ページ指定は「単独で表示（以降を組み直す）」「隣を白紙にする」と指定なしの「自動」。
  白紙は指定実ページの空き側で、独立ページ/ID/sourceにしない。Single/Split/実ページ数/出力に含めない。
- 組ごとの手動`SingleCompanion::FrontCoverSupplement`指定と編集操作は持たない。
  構成あり本も「末尾の単ページへ表紙を添える」の全体既定/既存本別preferenceに通常本と同じ規則で従う。
  添える表紙は構成適用後の先頭可視ページから派生し、recordへ補助指定を保存しない。
  末尾の明示BlankOppositeは白紙を優先し表紙を添えない。自動強制単独の白紙は従来どおり（§5.1）。
- 操作ごとの組変更確認は廃止。右へ結果を直接表示しdraftの元に戻す/やり直すで戻す。
  保存前の着地説明・解除確認・外部変更検証は別の安全確認として残す（§8）。
- 閲覧modeは独立。direction/cover/ページ指定はeditor、一ページずらしは構成本で使わない。
  端配置/表紙次・最終単独は自動規則として使い既存設定変更も許可（§9.1）。旧Q2/Q11/Q17の一律lockをこの範囲で改訂。
- 作成時は既存mIVの本別/既定/文書方向からdirection/coverを読み取り初期化する。
  組・shift・端配置・強制単独・補助を保存recordへ凍結しない。saved shiftは休眠保持、解除時はEの規則で再評価。
  released settings/spread/resume/しおりを移行不要として破棄しない。

EPUBも今回対象。既存個別編集と同じ元EPUB + 物理ページ番号を使う (§4.1)。
再変換でページがずれる場合は既存編集と同様に構成もずれる仕様を利用者が了承した。
**2026-10-12 利用者決定**: 独立段階G4Fを切り離し、画像だけのフォルダは今期の構成対象外とする。
ZIP/PDF/EPUBと、既存folder閲覧・個別編集・sidecar v1・metadata transferを維持する。
画像フォルダ→ZIPは[独立計画](image-folder-to-zip-plan.md)で現treeに実装されており、B1で変更しない。
Bでfolder構成adapter/sidecar v2を復活させない。
製本フォルダも引き続き構成対象外。対象範囲の旧G4F記述は将来案として読み、現行契約は本決定を優先する。

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

### 2.1 B1のコード照合（2026-10-13、71a3c2228）

以下はソース確認。実機観測は利用者の「片側非表示でも元の側に残る」「変更ごとの全体確認がスクロール表示され使いづらい」。
B1でアプリを起動・再現していない。まず通常builderの共用を調べ、別の構成用組立てを残さない方針とした。

| 現行経路・symbol | 確認した事実 / Bの変更 |
| --- | --- |
| `book_composition/model.rs::BookComposition/CompositionUnit/validate` | direction、cover PageRef、units、hiddenを保存。unitsが順序/組/配置/補助の正本。新recordへ置換（§4） |
| `resolve.rs::resolve_composition/project_pair` | 片側Hidden/MissingをLeading/Trailing Single、新tailを中央Singleへ。片側寄りはここで作られる。新resolverは可視実列/指定だけを解決 |
| `projection.rs::project_composition/AcceptedCompositionProjection/remap_removed_items/capture_initial_composition` | visible_pagesでrowsを並べunitsもreindexし、raw位置表/ordinalを作る。位置表は実列から維持し組凍結/unit remapを撤去 |
| `ui_fullscreen.rs::build_spread_display_units_with_page_alone/append_spread_display_units_until`（15541/15929） | 隣接nav、横長/非pairable境界、cover、PageAloneSettingsを処理。ページ指定を境界入力へ足せば同じ走査を使える |
| 同`compose_spread_navigation_unit/resolve_spread_display_composition`（14958/15042） | 組生成と配置解決は別。強制単独は白紙側、自然な先頭/末尾Singleはendpoint、横長は中央。presentation resolverも共用が必要 |
| 同`build_spread_display_units_for_nav/from_composed_unit`（17655/14721）、`continuous_reading_layout`構成branch | 現状は保存units専用でlandscape epoch=0。別組立てを撤去し共通出力をpaged/連結/Remote/editorへ |
| `app.rs::*_enabled_for_current_book`（26069/26077/26094） | 本別preferenceのeffective(global)で端/単独を解く。構成の固定組branchは再生成に使わない。Bは同じ実効値を入力 |
| `settings.rs::Settings`、`compose_spread_navigation_unit` | final_cover_spread_enabledは表紙の写しを末尾へ追加（既定true）。中央/片側やmembership設定ではない。構成本も通常本と同じ実効値/条件で適用。保存する手動補助と派生presentationを区別 |
| `app/book_composition_editor_prepare.rs::prepare/read_initial_binding/resolve_initial_binding/freeze_automatic` | mode-onlyを外してexact/fallback/既定/文書方向からbindingを読む。direction/cover初期化へ再利用、freeze/補助/shift取込みを撤去 |
| `book_composition/editor.rs::preview_edits/preview_rebuild`、`app/book_composition_editor.rs::editor_view`、`ui_composition_editor.rs::EditorChangePreview` | units差分から候補確認を生成。全体確認の原因。draft/undoを再利用しpending_preview/組差分確認を撤去 |
| `projection.rs::position_for_projected/resolve_position/reading_ordinal`、App editor保存着地/resume、`app/book_composition_remap.rs` | 永続identity/raw互換は組から独立可能。unitsのanchorを共通Navigation anchorへ置換（§6） |
| `ui_fullscreen.rs::composed_blank_spread_side`（29602）、`view_trim::stored_view_trim_bbox` | ResolvedUnitのBlankを探してQ8の側を解く。新指定/accepted presentationから側を取り、通常本trimを維持（§11） |
| `remote_ipc/container.rs::spread_payload`、`ui.rs` binding admission、`spread_shift/model.rs::resolve_book_layout_source` | Remoteは通常builder/管理flagを使い構成投影は未接続。Bではorder/hidden後の列を既存件数/byte制限へ通し、制限後の列+指定だけを共通builderへ渡す。切詰めPC parityは要求しない（§2.2/§10.2） |
| `SessionHandle::with_local_composition_admission`（session.rs:1663）、`poll_remote_session`（ui.rs:1194）、App editor owner/tickets | G4で開始/保存と取得barrierが実装済み。旧G0の「helper/ticket未実装」は履歴でありBの追加作業ではない |
| `book_composition/permission.rs::CompositionWriteAdmission::validate` | binding_scopeに構成行があれば一律拒否。許可する自動端/単独writeは型で目的を区別し、安全なscope/token/実体資格とHidden拒否を維持（§9.1） |
| `metadata_transfer.rs::PortableCompositionScope`（584）、`FORMAT_VERSION`（54） | BookCompositionを埋め込むため新recordへの変更必須。現treeはv11、旧§2のv9調査を前提にしない（§7.3） |
| `book_composition/store.rs` relative参照改名（463）、service/store JSON、rename/content restore/settings reset | units/cover/hidden走査をorder/hidden/override keyへ置換。scope/key/単一writer/新JSONの共通検証を維持 |
| `grid_selection_indices`、`book_composition_permission`、出力/製本受付 | 選択出力はitems順。全冊追加のprojection接続はG5受入事項。組はコピーせず構成payload転送と区別（§11.3） |

| 実際の設定 | 構成本への適用 |
| --- | --- |
| singleton_spread_first_enabled / last_enabled | 自然な先頭/末尾Singleを片側へ。cover/direction、complete book・UnpairedSlot限定は通常と同じ。横長を片側へ押し込まない |
| page_after_cover_alone_enabled / last_page_alone_enabled | coverあり・2枚以上・complete book限定の強制単独/白紙。coverなしでは無効。後続組替えも共通 |
| final_cover_spread_enabled | 通常本と同じ本別FinalCoverSpreadPreferenceのeffective(global)を使用。ON/OFF優先、FollowGlobalは全体値。構成適用後の末尾Singleへ共通条件で表紙を添える（§5.1） |
| 本別direction/cover・全体既定・文書方向 | 新規作成時の初期値のみ。保存後はrecordの明示値が優先。上の端/単独規則は凍結せず実効値に追従 |

**説明訂正と利用者再決定（2026-10-13）**: 設計担当が「表紙の隣に表紙の写しを薄く出す」と誤説明していたため、旧「補助不要」の判断とB1の構成本限定不適用を取り消す。
実際の手動補助は末尾Singleへ表紙を添える自動機能を組ごとにしたもの。正しい説明後、利用者は構成本にも通常の末尾表紙規則を適用すると決定。
`spread_db::get_final_cover_spread_preference_with_fallback`（368）はexact→fallback→FollowGlobal、
exactの明示FollowGlobalはfallbackを遮る。`FinalCoverSpreadPreference::effective`はOn/Offが全体値より優先。
表不在も既存のFollowGlobal扱いとし、新しい本別値/構成用設定storeを作らない。
PCの`final_cover_spread_enabled_for_current_book`とRemoteの実効値解決を同じ既存規則へ接続する。
B1でG5完成や実行成功を主張しない。

### 2.2 B1独立レビュー指摘の再照合（5e8049f5f、P2-1/P2-2）

P2-1は§4.2/§5.1/§8/§11.1/§13で明示白紙優先へ訂正。設計担当決定は§14のQ25。
自動白紙を表紙へ置換する現行resolverには明示/自動の形成理由を渡す必要がある（§5.1）。

P2-2の現行経路を確認した（製品起動なし）:

| コード | 成立している境界 / Bの扱い |
| --- | --- |
| container.rs::enumerate_zip（5937–5984） | source_itemsのメタ情報列から件数/byte budgetでbounded_itemsを作り、その列をspread_payload_with_readへ。Bはorder/hidden後の列に同じ制限を掛ける |
| 同enumerate_pdf（6069–6111、EPUBも共通） | page countから件数/byte制限内の行を作り、そのitemsでcompleteを判定。構成順のPageRef/番号と可視件数はpureに解けるため、制限前の全寸法準備は不要 |
| 同spread_payload_with_read（6313–6349） | Singleは横長判定なし。それ以外は返却itemsをcached_landscape_flags_with_readへ渡し同じitemsでgroups生成。full source_itemsはmetadata/identity用であり寸法入力にはしない |
| ui_fullscreen.rs::build_remote_spread_page_groups_with_shift（15876） | incompleteなら自動page_aloneをdefaultへ、共通presentationでもendpoint/final_coverをcomplete条件で抑止。Bでもこの境界を維持 |
| container.rs::cached_landscape_flags_with_read（7047）、collect_zip_entry_dims（2401） | 既存カタログ寸法を読み、渡されたitemsの未登録entryだけheaderを読む。full列へ拡大すると省略ページの追加I/Oを招くためBでは拡大しない |
| 同catalog_free_dims（6958） | PDF/EPUBの既存page-size APIは必要時に文書全体のsizeを返す。これは既存経路であり、Bで呼出し条件/範囲/APIを増やさない。「省略ページに一切読取がない」とは主張しない |

[§1.368](next-release-backlog.md)は次の版の後で検討する延期済みの既知制約。
例（指定なし、可視列0..4、LTR表紙あり、after-cover On）: PCは[0],[1],[2,3],[4]、
先頭4件へ切詰めたRemoteはcomplete=falseで[0],[1,2],[3]になり得る。
構成ありでも同じ制限後列/complete境界を使い、この差をBで全本生成・相方追加取得・group削除で直さない。
方針を成立させないコード上の障害は確認されなかった。全体のidentity/順序・可視件数の解決と、
返却範囲の寸法/組生成を分ければ既存経路を再利用できる。§1.368の解決時に構成あり/なしを一緒に直す。

## 3. 所有境界と処理順 — items生成時の一回の投影

新規 `book_composition` にpure model/resolver、`book_composition_store` に永続owner、
`app/book_materialization` に本の列生成境界を置く案とする。
既存 `src/materializer.rs` は外部受け渡しの実体化であり、本の列生成とは責務が異なる。

```text
source enumeration / ScannedDir / ZipTree / PDF page manifest
  → 安全性検証・file_type_visibility・既存同名除去
  → BookScope + 元の名前/ページ番号順 + メタ情報
  → materialize_book(snapshot, composition)       ← 唯一の構成適用owner
  → PreparedBook { items, metas, identity_index, page_overrides, binding, complete_proof }
  → 共通見開きbuilder / presentation（組は派生cache）
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
- indexへ解決済みのページ指定、direction/cover、構成revision、visible-page complete proof。
  hiddenも「登録済み」と数え、後から新ページとして再追加しない。
- 元manifestにあるが構成に未登録のページを元順で末尾へ追加する派生tail。
  既存末尾と同じ実列で自動pairingする。末尾中央Singleへ固定する旧Q3契約はBで廃止。
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
| 画像本フォルダ | 今期の構成対象外。production folder listing/既存Book自動表示・OrdinaryListを維持。ZIP化は独立計画 |
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
| 書庫実効階層/PDF/EPUB/適格画像本自身 | Book。可視ページは構成順、組は共通自動builder、詳細sort固定。構成なしは既存自動規則/§1.364 |
| Book内の一時facet/一覧内絞り込み | Bookのsubset。残る実列から再生成、解除で全可視列へ。端設定のcomplete条件は通常と同じ |
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

本扱いOFF/混在のOrdinaryListではlayoutを適用せず、適格Bookへ戻れば再適用する（Q12/13）。
構成が有効な書庫/PDF/EPUBのscopeのrecord bindingは§9.1で保護する。
画像folderは今期対象外なので保存試作行があってもbinding受付/提示/管理表示へ使わない。
これらの資格判定はworkerの実体分類で行い、全leaf構成依存やhidden markerを作らない。

結果から本へ入り直すことを強制しない。任意の「本を開く」は明示操作時に対象を解決し、
実際にBookへ移れる対象だけで案内する。本扱いOFF/混在元folderを開き直せば構成表示になるとは説明しない。
通常フォルダ詳細sortは抽出列扱いにせず、既存sortを保持する。sort lockはBook && accepted compositionのみ。
分類/layout/sort/操作資格は同じaccepted recipeから派生する。抽出列のshift資格のための
「構成本のleafが一枚でもあるか」の走査・scope依存は廃止する。

### 3.2 items行変更も投影ownerの更新である

整数slotはitems世代に束縛した派生値。order/hidden/manifestから可視実列・status・override対応を作り、
共通builderがその列から組を作る。保存recordではなくcontext cacheが組を所有する。

- 実削除: 既存old→new remapをitems/metas/raw位置表・identity/status/指定対応へ同時採用し組cacheを失効。
  `[A,B],[C,D]`のB削除は`[A,C], Single(D)`（縦長・表紙なし・指定なしの例）。元slot/Pairを保持しない。
  DBのB参照/指定はMissingとして保持し、F5前にもnav/resume/出力は新投影を使う。削除をhiddenへ保存しない。
- 改名: order/hidden/overrideのPageRefを一緒にmigration。scope外移動はMissing保持。
  再列挙が必要なら既存reloadで一括adoption。旧投影はholdover専用で旧identityの編集/出力を確定させない。
- 追加/外部変更/F5: 未登録実ページを元manifest順で末尾へ足し自動組へ含める。読取だけでDBへ保存しない。
  Missingが戻ればorderの元位置とその指定へ復帰する。
- policy/facet: Hidden/Missing/種類除外/一時subsetをstatusで区別し、組へ偽ページを補わない。
  Bookは残る実列で組を再生成。通常/抽出は従来結果順とQ16の遅延提示許可を維持（§5.1）。

accepted snapshotはitems/metas・raw/identity/status表・可視実列・指定対応・complete proof・依存revision/世代を一体保持。
寸法/回転/実効規則変更も共通cache失効へ通す。新入力と旧組を混ぜない。削除/改名/importは関係Book退避依存だけ通知。
read-only fork/mount/closeは通知しない。保存中のdraft順をviewer itemsへ差し込まない。

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

### 4.2 構成record（B方式）

論理形（名前は実装時に既存型へ合わせ、DBのscope/domain wrapperは維持）:

```text
BookComposition {
  schema_version, revision,
  direction: Ltr | Rtl,
  cover_enabled: bool,
  page_order: [PageRef],
  hidden_pages: set<PageRef>,
  page_overrides: map<PageRef, PageOverride>
}
PageOverride = Alone | BlankOpposite { page_side: Leading | Trailing }
```

上記だけを保存する。CompositionUnit、Pair、cover PageRef、独立placement/SingleCompanion、shift、
環境設定のコピー、寸法、items index、組番号は保存しない。page_orderが登録実ページ順の唯一の正本。
非表示/Missing/種類除外も参照を残す。初期値は本全体manifest（現在facet subsetではない）。
未登録実ページは元manifest順で末尾へ追加しeditor draftへも含め、利用者保存時だけorderへ定着。

- direction変更は実列を逆転せず画面左右/Split読み順を変える。
- coverありは並べ替え・非表示等の適用後、builder入力の先頭実ページを表紙とする。
  元表紙を隠せば次の実ページが表紙。表紙を替えるには左の順序を変える。
  subsetの表紙位相も通常builderへ揃え、completeでない列を完全な別本へ扱い直さない。
- hiddenはorderの部分集合。隠れたページのoverrideも保持し再表示時に適用。Missingも指定を消さず別identityへ代用しない。
- override不在が「自動」。Alone/BlankOppositeは排他的なenum一つ。
  AloneはNavigation実ページ一枚の中央Singleを作り、直前の未完成Pairを閉じ、次から通常の組を再開する。
  BlankOppositeも同じ境界で、一実ページを指定側へ置き反対側を白くする。
  既定Leading（LTR左/RTL右）、side選択は白紙指定の従属項目。独立した中央/片側指定は作らない。
  coverは先頭roleで扱い、そこにoverrideがあれば基礎配置はoverride優先だが表紙roleは変わらない。
  これらは組生成/基礎配置の指定。末尾表紙が適格ならその後に共通presentation resolverで表紙を添える。
  Aloneも末尾なら添付対象。ただし明示BlankOppositeは白紙と指定実ページ側を優先し、末尾表紙を添えない。
  自動after-cover/last/ForcedBoundaryで付いた白紙だけは通常本と同じく表紙へ置換できる。
  明示/自動の由来を共通builderの単独形成理由へ載せresolverまで渡す。指定recordは変更しない（§5.1）。
- 横長は指定なしで通常どおり中央Single。明示BlankOppositeなら片側+白紙、強制Pairにはしない。
  横長をPairへ強制する追加指定は不要（通常規則と競合し複雑になる）。「自動」は指定解除であり追加overrideではない。
- 白紙にID/選択/sourceなし。指定実ページを隠せば白紙も出ない。連続する実ページへの白紙指定は許す。
  旧slot展開の連続白紙禁止は独立白紙列を作らない本方式に不要。白紙だけの組/二つの相方/手動補助は保存型に表せない。自動末尾表紙のpresentation roleは派生値として使う。

種類/scope、relative/entry安全性、order正規化identity重複なし、hidden/override keyがorderの部分集合、
未知variant/字段/schema、既存件数/文字列/JSON bytes上限を共通検証。mapのwire表現は重複keyを検出できるものにし、
deserialize時の後勝ちで潰さない。実体欠落はMissing statusで、保存データを掃除しない。
未公開record/DB/構成転送payloadは新形式一つへ作り直し、旧units移行・推測変換は不要。
B1では実DB/試作データを削除しない。released settings/spread/resume/しおりは維持する。

### 4.3 編集操作と初期化

新規は既存reading resolverのexact→fallback→既定/文書方向からdirection/coverを初期化。
resolve_initial_bindingのmode-only分離を維持し、Single/Split閲覧中も裏のcover指定を取る。
構成ありは保存recordからdraftを作り、全体bindingを上書きしない。自動組・端/単独・補助・shiftを凍結しない。
shift中の初回作成で組が変わることは右の結果と短い説明に示すが、組差分承認は増やさない。
初期化/取消/保存は全体既定・既存spread/shift storeへ書き戻さない。

左の移動/hidden、上部direction/cover、右のPageOverrideは一つのdraft操作として検証後即適用。
右の対象は組番号でなくPageRef。そのページ指定だけを替え後続を共通builderで再生成する。
候補draft/組承認phaseは持たない。undo/redoはrecordとPageRef選択の在メモリcheckpoint。
派生組を履歴へ保存せず、戻したrecordと採用済規則から再生成。選択identityを追い全体確認/先頭への強制scrollなし。

準備workerで全manifest/安全性/保存回転/必要寸法/実効自動規則を取得。editor modalでsettings変更を止め、
編集中は同規則snapshotを使う。大冊の再生成はeditorの結果生成worker経路へdraft世代を渡し最新版だけ一括採用。
この繰り返し結果生成は現treeの一回prepareとは異なる後続実装で、editor sessionが要求を一つ所有する。
複数candidateを保持せず未処理入力は最新draftへ集約し、送信中の結果は世代照合で採用する。
旧結果は更新中と分かる表示にし保存根拠にしない。UIで全件decode/走査/DB待ち、別pending bool/復旧ownerなし。
viewerでは通常どおり寸法/回転/実効規則変化に追従（§5.2）。解除は全指定を外し既存本別/既定/shift resolverへ戻す。

## 5. Spread / Single / Splitとキャッシュ

### 5.1 builderの入力と出力 — 組を作る場所は一つ

```text
元manifest + order/hidden
 → 共通Book projection（可視実列・PageRef→idx指定・status/raw位置表）
 → 通常の共通見開きbuilder（direction/cover・実効自動規則・寸法/回転・ページ指定）
 → 共通presentation resolver（左右/中央/白紙・Navigation anchor）
 → editor結果 / paged / continuous / Remote PageGroup
```

構成resolverは実列と指定を解決し、組立ては持たない。通常のbuild_spread_display_units_with_page_alone/
append_spread_display_units_untilをページ指定の境界入力へ拡張する。通常本は空overrideで現出力を保つ。
切出しが必要なら既存pure関数を移しadapterを残す。Composed/Automaticのタグは入力所有/資格/shift可否であり、
組アルゴリズムの二択にしない。ResolvedCompositionもunits不要の可視実列/status/指定となる。

1. nav順を使いhidden/Missing/種類除外を前段で外す。白紙slotで穴埋めしない。
2. 指定ページの前segmentの余りをSingleで閉じ、指定Singleを一つ生成、次から通常隣接Pairを再開。
   次ページが指定付きなら相方にしない。自動cover/after-cover/lastも同じ境界走査で一度だけ処理し、
   segmentごとに先頭/末尾規則を繰り返さない。
3. 指定なしは通常規則。明示境界→自動page_alone→横長/non-pairable→隣接Pairの優先で確定。
   横長に隣接する余りも通常の原因/中央配置。cover自身は先頭Singleを一度だけ作る。
4. 基礎配置は明示Aloneが中央、BlankOppositeが指定側+白紙。自動after-cover/lastと重複しても一Singleで明示基礎配置を優先。
   明示境界直前の余りは中央（自然な本の先頭/末尾なら通常endpoint規則）。
   自動page_alone直前の余りは既存ForcedBoundary配置。残りは通常presentation resolverの端/強制単独を適用。
5. 最後に、通常本と同じ実効final_coverと適格条件で末尾Singleへ表紙を添える。
   Navigation membership/組数/anchorは変更せず、表示だけにSpreadPageRole::FrontCoverSupplementを加える。
   明示BlankOppositeなら白紙を優先し添付しない。自動強制単独の白紙は従来どおり表紙へ置換。
   組ごとの手動補助指定は生成/保存しない。

complete proofは「scopeの採用orderからhidden/Missing/policyを適用した論理可視本全体」が基準。
hiddenだけで末端設定を無効にするnav数=元manifest数の判定は使わない。facet/検索subsetやRemote切詰めをcomplete扱いしない。
Book subsetは残る実列から再生成し、complete限定のendpoint/page_alone/final_coverは通常と同じく抑止する。

**構成適用後の末尾表紙判定**（共通presentation resolver一か所。Remoteの制限後入力は§10.2）:

- 順序/hidden/Missing/policyを適用した可視実列を共通builderで組み、その入力列の最初/最後のNavigation unitを使う。
  Remoteでは§10.2の制限後入力列であり、全本のunitを別途準備しない。
  保存orderの最後、旧unitsの最後、最後に操作したページ、Remote応答の最後から末尾を推測しない。
- Spread閲覧、complete book、構成のcover_enabled=true、実効final_cover=true、
  先頭unitと最後unitがともにNavigation実ページ一枚で、末尾実ページが表紙と異なり、
  末尾に明示BlankOppositeがない場合だけ添える。
  最後が実ページPairなら相方は埋まっているので添えない。0枚/表紙だけの1枚、coverなし、
  Single/Split/端末single強制、subsetは添えない。横長由来の末尾Singleにも通常本と同じ条件で添える。
- 空き側は「二枚目のNavigation実ページがない側」。ただし明示BlankOppositeの側は利用者が白紙に指定した面なので添付対象にしない。
  自動強制単独の白紙だけは従来の空き側扱いを維持し、設定に従い表紙へ置換する。
  coverは現在の可視実列の先頭で、非表示/欠落/種類除外となった旧表紙を別sourceとして呼び戻さない。
- PC/editor/連結は採用済みの本全体またはそのview入力で判定。Remoteは構成のorder/hidden適用後に
  通常と同じ応答制限を掛けた列で生成し、切詰めならcomplete=falseで末尾表紙を抑止する。
  切詰め前の全本の組/寸法を準備しない。完全応答のみ本全体の条件となる（§10.2、§1.368）。

| 構成適用後の末尾状態 | 実効末尾表紙ONかつ上の適格条件成立時 | OFF/条件不成立時 |
| --- | --- | --- |
| 通常の端数/横長Single | 共通resolverの通常配置で表紙を添える | 通常の中央/端配置 |
| 最後の元ページをhidden/Missing/policy除外 | 残りを繰り上げて再生成した真の最後がSingleなら添える。Pairなら添えない | 再生成結果の通常配置 |
| 末尾にAlone指定 | Navigationは一枚のまま表紙を添える。通常のNavigation-first画面順を使用（LTRは末尾実ページ左/表紙右、RTLは鏡映） | 指定どおり中央Single |
| 末尾にBlankOpposite指定 | 明示指定を優先。指定実ページ側+白紙を保ち、表紙を添えない | 指定実ページ側+白紙 |
| 自動after-cover/last/ForcedBoundaryによる末尾Single+白紙 | 通常本同様、実ページ側を保ち白紙を表紙へ置換 | 既存強制単独の側+白紙 |
| 非末尾のAlone/BlankOpposite | 添えない。指定の中央/白紙を維持 | 同じ |

Aloneは実ページ一枚の組を作る指定で、末尾表紙を抑止しない。BlankOppositeは反対側の白紙も明示するので末尾表紙より優先。
その末尾だけ白紙にしたければBlankOpposite、本全体の自動末尾表紙を使わないなら既存本別preferenceをOffにする。
独立した「補助なし」フラグは増やさない。

**P2-1: 白紙の由来をresolverまで保持する**:

現行compose_spread_navigation_unit（ui_fullscreen.rs:14995付近）はhas_white_companionを見て自動白紙を表紙へ置換する。
同じLeftWhite/RightWhiteへ明示白紙を潰すと判断不能になるため、既存SpreadDisplayUnitFormation/SingletonCauseを
排他的に拡張する（論理案: ExplicitAlone / ExplicitBlankOpposite(side) / 既存ForcedAfterCover・ForcedLast・ForcedBoundary等）。
PageOverrideから共通builderが形成理由を生成し、共通presentation resolverはその同じunitの理由を読む。
ExplicitBlankOppositeはfinal_cover分岐へ入れず指定側+白紙のpresentationを確定する。
同じページが自動強制単独にも該当しても明示由来を採用し、設定On/Offやcomplete値で由来を変えない。
独立bool、field presence、白い面の有無だけで由来を推測しない。保存型は既存PageOverrideのままで新字段なし。
PC/editor/連結/Remoteは同じ形成理由で処理し、cache/holdoverもそのaccepted入力・presentationへ従う。
構成なし本はExplicit由来を作らず、既存自動白紙→表紙の条件・配置を維持する。
右の自動結果にも派生表紙を表示し「末尾に添えた表紙」と区別する。
明示BlankOppositeの結果は指定側+白紙のまま、添付表紙を出さない。独立選択/移動/override対象にはしない。
添えた表紙はNavigation/seek/実ページ数/resume/出力targetへ加えず、元の表紙の編集/trim/sourceを通常経路で読む。
新しい添付sourceにも提示許可とsource readinessを要求し、一unitの二画像をatomicに提示する。

例（縦長、表紙なし、端/強制単独OFF、LTR）:

| 実列/指定 | 派生結果 |
| --- | --- |
| A B C D E、自動 | [A,B] [C,D] Single(E) |
| B非表示 | [A,C] [D,E] |
| BにAlone | Single(A) Center(B) [C,D] Single(E) |
| BにBlankOpposite(Leading) | Single(A) [B,白紙] [C,D] Single(E) |
| B指定解除 | [A,B] [C,D] Single(E) |
| B横長、自動 | Single(A) Center(B) [C,D] Single(E) |

direction変更は実列/membershipを逆転せず画面左右を鏡映。Singleは同実列、白紙なし。
Splitは同実列を既存page_split::PresentationStepへ。端末portraitSinglePage/force_single_pageも投影だけ。
連結/page-edit pivot/seek/履歴/実ページ数は共通unitのNavigationだけ。白紙にtexture/source/readinessなし。
一/二実ページのatomic提示、Eのfit/geometry resolverを維持し式を複製しない。

**Q16の通常/抽出結果列の例外は維持**: 元本のorder/overrideを借りず結果順の自動組。
新規提示候補だけhidden照合し、片側Hiddenは元slotのSingle、両側はskip。他unitの相方を借りない。
結果列まで全件繰り上げると全leaf lookupが必要で承認済Q16に反するため採用しない。
今回の繰り上げは構成を適用するBook/editorが対象。明示targetはtoast拒否、送りはskip通知、全Hiddenは一覧。
結果thumbnail/selection順は維持し、保存時refresh/hidden marker/全列skip cacheなし。

### 5.2 キャッシュのキーと失効

immutable構成snapshotと既存scope/service epoch/publication sequence、manifest/policy/items世代、context所有を維持。
組cacheは可視nav・mode・direction/cover・指定対応・構成token・landscape epoch・実効page_aloneを含む。
presentation/geometry keyは実効endpoint/final_cover、添付表紙identity/role、寸法/回転/trim/scaleも含む。
構成だけlandscape epoch=0にするbranchと初期規則凍結を撤去。寸法後着/回転/設定変更で組が変わる場合は
通常cache失効/identity reanchorを使いDB record/revisionは変えない。専用live-rebuild ownerを足さず既存規則更新/reloadへ。
新列/指定/規則と新組を一括採用しidx一致だけのtexture/fit再利用を禁止。

保存/解除/importは関係Bookだけ既存reload、退避Bookは復帰adoptionで鮮度照合（§10.1）。
結果membership/navは構成保存で失効させない。Q16の提示許可は要求ID/context/items世代/identity/occurrence/scope tokenを照合。
連結新入場も§6.2の既存demand owner。cached texture/prefetchは許可を兼ねず、Hidden欠落をbase unit cacheへ焼き込まない。
同episodeのみ許可保持、退出/closeで破棄。新unitの新相方（末尾へ添える表紙を含む）は許可前にfit/paintへ採用しない。
旧holdover/passive snapshotは捕捉unit/role/geometry/tokenを保持し現在規則で再描画しない。
source/role/slot/sliceが変わればそのcontextのfit参照/許可を失効。read-only mount/fork/closeは兄弟cache/queue/cancelへ作用しない。

## 6. 読書位置・しおり・履歴と非表示target

Bでも永続位置は実ページidentity。組番号・units内offset・白紙位置を保存しない。
旧units依存のposition/ordinal/remap接点は§2.1。可視実列から同じraw互換表/ordinalを作り直す。
再開/しおりのtargetを先にidentityで可視indexへ解き、含まれる組のNavigation anchorを共通builderで解く。
組替えでtargetが新Pairの二枚目になってもその実ページを含む組へ着地し、旧組番号の別ページを開かない。

構成後のindexは永続identityへ直して記録する。現在のresume writerと
`PreparedPageIdentityIndex`を使い、構成保存後は同じidentityを新indexへ解決する。
構成本のrecordでは、互換用raw値を**構成前の正規列index**へ変換してidentityと同時保存する。
HUD/meterのordinalは構成後の可視列から別に算出する。rawに構成後indexを入れると、
旧版書込trigger・generic key移行でidentityが失効した場合に元列の別画像へ着地してしまう。
在メモリdedupの表示idxと、writerへ渡す互換raw値を混同しない。
派生組の左右や白紙からanchorを逆算せず、共通builderのNavigation anchorの実ページidentityを保存する。
実ページ数/ordinalの分母は可視実ページ、白紙/添えた表紙の再出現/子containerを含めない。
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
   workerはその画面で新たに提示する候補だけ照合し、同じsourceの添付表紙/sliceは一要求内で照合を共有できる。
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

Single→Spread、Split切替、末尾へ添える表紙の新しいoccurrenceも同じ **presentation demandの差分** で受け付ける。
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
JSON内は相対PageRef/物理ページ番号のみ。新record全体を一transactionでreplace/deleteする。派生組のテーブルは作らない。
一覧は数万ページを想定し、worker一括照合 O(n)、索引map O(n)、JSON上限をschema/APIで共通検証する。
新ストアは未リリースなので**新ストアの旧schema migrationは不要**。
ただし既存released settings/resume/spreadや構成sectionのない旧portable bundleまで移行不要と解釈しない。

App所有の単一service/workerがwriter。UIはPrepare/Save/Remove要求とreplyを扱い、同期SQLite/走査を追加しない。
読み取りsnapshotはimmutable Arc、contextごとに採用済revisionを保持する。
rename/purge/import/content restoreも同じmaintenance直列化境界を使い、第二writerで同時に書かない。
既存共通maintenanceがDBを直接更新する期間はserviceをquiesceし、完了後snapshotを更新する。
UIスレッドでjoin/lock待ちをせず、既存maintenance workerの完了を受ける。

保存失敗はtransactionで旧recordが残る。editorが存続していればdraftを表示したまま「本の構成を保存できませんでした」を通知し、
利用者の再操作かキャンセルへ戻す。自動retry、recordの途中保存、独自quarantine/複数段回復を追加しない。
owner終了/Remote取得で既にeditorを閉じた場合は通知だけで、draftを復活させない (§10.2)。
破損/未知schemaならDBを作り直さず、通知・書込停止・構成本の閲覧拒否とする (Q7決定)。
構成は利用者作成データなので、再生成可能なサムネcacheと同じ扱いにはできない。
稀なcommit後reload失敗は保存済recordを保持、通知して既存再読込または再起動を案内する。
表示を新構成で完成したと偽らず、失効した旧indexで編集/移動を続けない。
これ以上の回復が必要というレビュー指摘は、先に利用者へ割り切り案と費用を相談する。
明示「今回は構成を使わず開く」を用意する (Q7決定)。
選択したopen要求のcontext/scopeに限るAutomatic投影で、DB row/他contextのaccepted構成を変更しない。
shiftは許可せず、構成編集も書込停止中は受けない。自動規則の本別writeも構成の存在/資格を安全に判定できなければ拒否する。
次のopenへ選択を記憶せず、独自の復旧状態を永続化しない。

### 7.2 登録先チェックリスト

| 接続先 | 必要な処理 |
| --- | --- |
| `rename_key_migration::STORES` | KeepDrive/uniqueな本行を登録し、exact/`/`/`::`prefix rename/copy/probe/purgeの対象へ。DB未作成は正常不在 |
| 画像個別改名/移動 | しおりの`migrate_paths_at`同様、該当containerのpage_order/hidden_pages/page_overrides key内のrelative PageRefをまとめてtransaction更新。page移動がscope外ならMissingを保持。root改名だけではJSON内変更不要 |
| archive内entry変更 | 本体から行う対応操作があれば同じPageRef移行を呼ぶ。外部rewriteはMissing保持で手動再対応。ページ番号から別entryを推測しない |
| 実ファイル削除purge | 本/rootの削除成功は構成行/prefixをpurge。ページ一枚削除では親構成をpurgeせず参照をMissingとして保持。非表示操作はFS/purgeを一切呼ばない |
| コンテナcopy / 内容ID復元 | generic `copy_stores_at`の本行とvirtual-prefix mappingへ参加 (`content_identity/restore.rs:414`)。relative/page-number payloadなのでcache mapping不要。destination既存構成を上書きしない既存conflict規則を使用。Folderには内容ID復元を約束しない (§4.1) |
| restorable probe / metadata cleanup | 構成も復元可能なuser dataとして数える。cleanupラベル、DB件数/clear、設定完全reset、data-dir移動・保存先説明へ登録。resetは既存明示確認の範囲で行う |
| 明示metadata export/import | §7.3の専用composition familyを追加。既存PortableContainerStateのorigin/挿入keyと分離。root/prefixはbundle相対、page refsは本相対。is_empty/has_view_state/summary/import changed flagsも更新 |
| metadata import refresh | owner/context/policyを捕捉し、関係する同一本scopeのBook projectionを既存reloadで更新。結果列は更新せず、別本のindex/cacheを捨てない |
| 自動sidecar | 今期の構成対象外。既存v1を維持し、§7.4の旧将来案をBで実装しない |
| F12 | registryのnew/default/destructure/swap/mount/fork/park/drop、親一覧/固定snapshotへprojectionを運ぶ。cache ownerはcontextごとに独立 |
| Remote | core engineにread snapshot/lookup登録、scope resolver・list/page context・visibility admission・payload登録。IPC版とWeb asset tokenを同時更新 |

構成保存/解除、rename/import/restoreが実際に変えたscopeだけに変更通知を出す。
読み取り専用open/closeはmutation-style invalidationを出さない。
content identityは元メディアの内容であり、構成revisionを内容hashへ混ぜない。
**構成保存成功時**には、要求時に捕捉したlogical rootへ `ContentIdentityTrigger::Edit` を送る。
現行 `record_current_container_content_identity` (`src/app.rs:25962`) の考え方を使うが、
reply時のmount/current_folderを参照しない。PDF/EPUB/ZIP/対応元書庫は新規構成だけの保存でも
restorable ledgerを作る。nested保存もouter logical rootを観測する。Folderは非対応として送信対象外。
editor保存/importによる構成作成は同じ成功通知へ接続する。Remoteにdirection/cover/order/hidden/overrideの構成write入口は作らない。
STORES/copy/probe登録のみで観測済扱いにしない。
純粋な本読みやmissing検出で永続構成を勝手に掃除しない。

metadataは現treeの未公開構成sectionを含むv11を新recordへ改訂。manifest/shardを一致させ、
v7–v10のsectionなし入力と既存字段変換を保持する。公開前に番号を確定し旧units変換の版を追加しない。旧bundleの構成不在は「既存構成の解除」ではない。
importは既存preview/競合方針に従い、新payload全体を検証してから本行単位で書く。
imported JSONのabsolute path/`..`/unsafe entry/過大payloadは拒否し、既存データを変更しない。

### 7.3 構成専用のメタ情報転送family

BではPortableCompositionScope.compositionを§4.2の新recordへ置換。
BookCompositionを埋め込む現コードのvalidator/JSON fixtures/件数表示/改名コピーを一体変更する。
hidden/Missing参照とoverrideも転送し、派生units/寸法/環境設定/白紙画像/表紙写しは転送しない。
受入先の実効自動規則から組を生成するので設定が違えば転送元とPair/端配置が異なる。
それは指定の欠落ではない。単独/白紙指定はPageRefに保持する。未公開v11の旧units section/DBに移行なし。
旧v7–v10の構成section不在は以下のkeep契約を維持する。

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
order/hidden/overrideを別々にmergeせず、本行は既存の編集・spread等と同じ置換規則へ揃える。

既存ImportConfirm一回に「本の構成: 追加○冊・置換○冊・解除○冊」と
「構成なしと記録された本では、取り込み先の構成を解除します」を加え、同じインポートボタンで一括承認。
本ごとのprompt、カテゴリ選択checkbox、別の構成置換承認画面は追加しない。
多数本・全構成なしのpayloadも一回。previewはworkerでread-only照合し、適用時は通常のtarget再検証を行う。
preview以後の変更があれば最終summaryの実適用数で説明し、per-bookの再確認へ分岐しない。
輸入元revisionは採用せず、受入先writerが実変更時にlocal revision/tokenを進める (§5.2)。
構成と既存spread/trimのtarget keyは別々に計算し、既存import batch/SAVEPOINTへ参加。
全件一transactionにはせず、cancelで適用済batchを保持する既存契約を維持する。
resume/履歴は転送しない。完了通知はBookに限定。将来用ImageFolder値の型/store/明示転送は保持するが
画像folderのruntime適用や§7.4のbackup/旧sidecar消費は今期対象外。

### 7.4 画像だけの本フォルダのサイドカー (旧G4Fの将来案・現行対象外)

以下は切り離し前の設計履歴。Bでsidecar v2/receipt/folder adapterを再導入しない。
将来再開時も旧units payloadは使わず§4の新recordを前提に別途採択する。

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
UI modal判定だけをRemoteの確定境界とみなさない。現treeの確定境界は§8.1の既存session helper。

画面構成（上部と下端操作行を固定し本文だけscroll）:

- 上: 本名、構成あり/新規、読む方向（右開き/左開き）、表紙あり/なし。閲覧mode/任意表紙ページpickerなし。
- 左: 「元ページ一覧」。全参照をdraftのpage_order順に示し元名/物理番号を併記。
  drag/前へ/後へで実ページ順を変更し、非表示/表示に戻すをここだけで操作・確認。hidden行も元位置に残す。
  Missing/種類除外は別理由。種類設定を変えたり除外画像をdecodeしない。
- 右: 「自動で組んだ結果」。可視実ページだけの組カード。順序drag/組移動/hidden操作なし。
  実ページを選び「自動」「単独で表示」「隣を白紙にする」を指定。白紙には実ページ側選択を従属させる。
  指定ページに「単独」「白紙・左側/右側」の文字badgeを出し自動Singleと区別。表紙印はcover由来。
  横長/自動強制単独は補足説明で示す。白紙は空き側の半透明/破線placeholder、独立選択/dragなし。
  末尾表紙も共通resolverの結果どおり表示し「末尾に添えた表紙」と説明。
  明示白紙は維持し、末尾表紙を添えない理由を説明する。自動強制単独の白紙だけは添付表紙に置き換わった結果を示す。
  添えた表紙には手動指定のbadgeを付けず、単独選択/移動/編集操作なし。
  「白紙は見開きだけに使い、単ページ・横長分割では表示しません」と説明する。
- 下: 元に戻す/やり直す、保存、キャンセル、構成を解除。両一覧の選択をPageRefで対応付ける。
  hiddenの保持済overrideを編集する場合は左で表示に戻して右で指定する。hidden印を右へ出さない。

| 今ある操作 | Bの残す/消す判断 |
| --- | --- |
| 読む方向・表紙指定 | 上部に方向・表紙あり/なしを残す。「このページを表紙に指定」は消す。表紙は先頭可視ページ |
| この2枚を組にする / 組にする | 消す。必要なら左で隣へ順序変更 |
| 編集中の条件で組を作り直す | 消す。編集後に自動生成して常に結果表示 |
| 前へ・後へ / 実ページdrag | 左だけ残す。右の移動は消す |
| 組単位の移動 / 組カードdrag | 消す。派生組に保存identityなし、順序ownerは左だけ |
| 非表示・表示に戻す | 左だけ残す。右にhidden行/操作なし |
| 単ページにする | 右の「単独で表示（以降を組み直す）」へ。相方も固定Singleにする旧Pair分割は消す |
| 中央・片側 | 独立操作は消す。単独指定の基礎配置は中央、白紙指定の側だけ選択。末尾表紙を添える場合の最終配置は§5.1、自動端配置は環境/本別設定 |
| 白紙の挿入・取り除き | 右の「隣を白紙にする」/「自動」へ。独立白紙の挿入なし |
| 組ごとの表紙の補助表示 | 手動操作/保存指定は消す。自動の末尾表紙は環境/既存本別設定から結果へ表示 |
| 元に戻す・やり直す | 残す。一操作一checkpoint、組変化一覧の承認不要 |
| 保存・キャンセル・構成を解除 | 残す。解除は保存指定全体を失うため確認 |
| 組の変更プレビューと適用/取消 | 消す。右へ直接反映。Escで候補だけ取り消すphaseも消す |

| 既存安全確認 | 判断 / 理由 |
| --- | --- |
| 操作ごとの全組差分 / 作り直し確認 | 廃止。直接結果+undo/redoで確認できる |
| 保存前の元窓の移動先 | 固定行付近の短い非blocking説明を残す。anchorを隠した時の次→前→先頭→一覧/兄弟窓の影響は組だけでは分からない。追加保存dialog/全体一覧なし |
| 構成解除 | 一回の確認を残す。保存済order/hidden/指定/direction/coverを解除し通常規則と休眠shiftの再評価へ戻る永続操作 |
| 外部manifest変更 | 保存workerの再列挙/fingerprint検証を維持。変化ならdraft保持・保存拒否・明示読み直し。別内容への誤指定防止、自動mergeなし |
| DB失敗/強制終了 | 既定ログ+通知/原本保持。存続draftと旧recordを保持し自動retry/永続draft/多段回復を足さない |

modal owner/Preparing→Editing→Saving→Reloading、start/save確定境界、Remote後取得優先、保存ticket、
兄弟mount/窓close終端は§8.1/§10.2を維持。組承認だけの第二modal stateを撤去する。
背面閲覧/slideshow/open/history/F12/context activate/別editor/settings/file操作/importは既存admissionで止める。
外部openを後から実行するqueueへ積まず通知して拒否。Remote取得は拒否せずOS別アプリ操作も制限しない。

二一覧をvirtualized/可視thumb予算付きworker取得。広い画面は左右、狭い画面は同じ二一覧をタブ化。
上部/下端操作行を常に到達可能にし、選択identity/scroll anchorを保持して全組へ強制scrollしない。
テーマ/未知glyph制限、Enter/EscapeのIME helperを維持。Escは解除確認を閉じ、確認なしならキャンセル。
新shortcutは追加せず将来はKeyAction/keymapへ。1093×614/1366×728、倍率100/200%、light/dark、長名/指定印/白紙のsnapshotを後続で更新する。

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
既存acquire (`:1544`) と同じinnerに入る。旧G0時点は提案だったが、現treeでは`SessionHandle::with_local_composition_admission`（session.rs:1663）が実装済み。
Bではこの既存helperとApp editorのowner/ticketを維持し、別開始gateを追加しない。

lock順は **UI routerの直列処理 → session.inner → O(1)のowner/ticket採用 + try_send → unlock**。
App側owner/ticket tableはUI router所有で新mutexを付けない。channel full/disconnectedなら同じ短い受付内で
未送信ticketを外しEditingへ戻して通知し、write済みとしてbarrierへ数えない。
inner保持中にui_request_admission/gui gate/registry mount/service lock/DB/flush fenceを取得しない。
workerはsession/registryのlockを取得せず、完了channelを既存UI routerが収穫してからphaseを確認する。
既存BeginAcquireのgui gate publicationから逆順でinnerへ戻る経路を新設しない。
start/saveの非blocking受付と、保存workerが行うmanifest検証/transactionを分離する。

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

## 9. §1.364との境界と閲覧中の操作（B方式）

BookLayoutSourceは入力所有の資格として維持。AutomaticはEの解決済shiftと本別/既定binding、
Composedはrecordのdirection/cover/可視列/overrideと実効自動規則。組アルゴリズムは共通。
Unreadableを構成なしと扱わない。抽出A2は引き続きbinding/shift禁止。

### 9.1 一律pairing lockをやめ、変更する値のownerで判断する

| 閲覧中の操作 | 構成ありBookでの扱い / 理由 |
| --- | --- |
| 一ページずらし（Spread）、§1.364保存/解除 | 引き続き不可。order/指定と別の組境界ownerを増やさない。必要な境界は右の単独/白紙指定と左の順序で編集。保存shiftは休眠保持 |
| Ctrl左右等のSingle/Split branch | 従来の一ページ移動を維持。shift拒否の前に通常navへ分離してhidden許可へ |
| 読む方向切替、HUD方向/表紙、方向/表紙を含むSpread/Split KeyAction | 引き続きeditorへ案内。recordを第二閲覧値でoverrideせず上部へ集約。editor外の小編集writerを増やさない |
| 単ページ/見開き/横長分割、連結読み、Remote端末single強制 | 維持。view-mode/flow/orientationだけ、同可視実列/directionを投影。構成writeなし |
| 先頭/末尾端配置（HUD、本別preference、Remote SetSingletonSpreadEndpointPreference等） | **許可へ変更**。共通presentation resolverの規則でunitsと競合しない。既存spread.db ownerへ保存し当該本の規則/cache更新。明示Alone/BlankOpposite優先 |
| 表紙次/最終単独（HUD、FsTogglePageAfterCoverAlone/FsToggleLastPageAlone、Remote SetPageAlonePreference） | **許可へ変更**。共通builderへ既存本別preferenceの実効値を渡す。coverなし/subsetは通常と同じ不適用。明示override優先、record変更なし |
| 末尾表紙（HUD、Remote SetFinalCoverSpreadPreference） | **許可**。通常本と同じ既存preference ownerへ保存し、実効値/共通presentationを更新。構成record/revisionは変更せず手動の組補助は作らない |
| Remote SetSpreadのページ構成/方向変更 | direction/coverを書こうとする結合要求は拒否。modeはSetViewModeで維持。order/hidden/overrideのRemote editor/APIは追加しない（PC限定Q6維持） |

組を保存しなくてもshift/record小編集を自動的に許す必要はない。既存PairingEditAvailabilityの一律拒否を
action種別ごとのpure資格へ改訂し、HUD/キー/カスタムキー/gamepad/IPC widgetと実setter/write admissionへ同じ判定を渡す。
現在のCompositionWriteAdmission::validateはbinding_scopeの構成存在を一律拒否するため、書込目的を
record bindingと自動規則へ型で分ける。workerの実体資格/scope/token照合とhidden target拒否は共通で維持し、
許可のために保護scopeを消した空admissionを偽造しない。
新owner boolを増やさない。構成なし本・通常画像folder・Eの§1.364は維持。
OrdinaryListで構成が休眠中のscopeもrecord binding変更禁止を維持し、layout不適用を理由にrecordを書き換えない。
A2は元本設定を借りない固定資格で、既存Spread/Splitキーは表示分類だけ一時routing。

構成indicator/editor導線を残す。理由は値に合わせ「読む方向と表紙は本の構成で設定されています」
「本の構成ではページごとの指定で組を調整します」。末尾表紙は通常本と同じ有効項目で説明する。
許可する自動規則まで無効化せず同値binding要求も拒否。全体direction/coverは保存recordへ上書きしない。
端/単独global値は本別FollowGlobalから実効適用。お気に入り等もbinding部分を持ち込まず無関係な設定/自動規則を維持。

### 9.2 閲覧modeをbinding変更から分離する

実装済みViewMode::Single/Spread/LandscapeSplit、mode-only保存とSetViewModeを維持。
既存spread.dbへ表示分類だけ保存、record/revision/direction/cover/全体既定を書かない。
Spreadへ戻れば現在構成+実効自動規則で共通builder。Singleはcover解除なし、Splitも同direction。
旧結合SpreadMode enumのdirection/coverビットを構成の正本にしない。
FsViewSpread/FsViewLandscapeSplit/FsSpreadSingle、HUD表示方法はmode-only。
方向/coverを含むキーは§9.1拒否、A2は分類だけの既存routing。旧Single/Vertical、exact/fallback/文書方向/flowの復元を維持。
Remote orientation/force_singleも投影だけ。結合SetSpreadのbinding部分を黙って無視して成功としない。

### 9.3 saved shiftとの接続

EのC1–C12/BookScopeKey/accepted snapshot/restore ownerを維持。作成/編集/取消/保存はshift rowを消去・更新しない。
**Bでは初期組へのshift取込みも行わない**。構成ありはshiftを解決/適用しない。
解除時だけ現在の記憶ON/OFF・scope・identity規則で再評価。OFFの休眠rowをsessionへ持ち込まずMissing anchorを同番号で代用しない。
shift DB移行/構成への複写/二DB同時commit/rollbackなし。
旧Gの「shift後のPair凍結」fixtureを新仕様へ替え、構成なしEの回帰は弱めない。

## 10. F12 / 多窓 / Remote

Bでbundle/retained projectionが運ぶ正本はimmutable record・可視実列/指定/実効規則・identity表。
unitsは各contextの共通builder cacheだけ。forkでimmutable入力を共有してもcache/channel/cancel/generationは独立。
§2.1の旧units consumerを置換し、App-globalな現在組やWeb専用組立てを導入しない。
Remote構成投影全体は現treeの未接続G5を含む後続実装事項。以下は新方式で成立させる契約。

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
旧G4Fのreceiptは復帰recipe/F12 bundleへ載せない (`330e070f6`の訂正)。Bではreceipt自体を実装しない。
本から離れている窓をsaveで強制移動させず、退避Bookは復帰時に更新する。

editor terminal closeは §8.1/§10.2に従う。indexを使うdraftを別窓に移植しない。
detached述語/viewportに触る前にはClaudeCodeと独立Codexレビューで
「context投影の所有を拡張する構造的修正であり症状パッチでない」ことに合意し、
実装範囲と理由を `detached-rework-plan.md` §11へ記録する。
新規detached bool、geometry捕捉、repaint/retry/time窓による修復は不要。

### 10.2 Remote

coreのContainerEngine workerが共通pure projectionで構成順/hidden/指定を解決し、
通常本と同じ返却範囲の入力からPageGroupを生成する。全本の表示unit/寸法を持つPreparedBookを要求しない。
Webは返されたentries/PageGroupを表示し、
構成のorder/hidden/overrideや自動組を自前で再現しない。実addressは元root+entry/物理PDF・EPUBページ番号のまま。
Bookには `has_composition`、revision/accepted layout identity、構成の実効bindingを追加する。
§9.1の操作ごとの資格/理由を返し、direction/cover/shiftは無効、自動端/単独/末尾表紙変更とmode-onlyは有効にする。
ExtractedResultには列種別/A2の固定操作資格/一時modeを返し、leaf hidden marker/構成依存を送らない。
結果のthumbnailはそのまま、full-page/ナビ候補をcoreで遅延照合してskip/拒否する。列種別から生成し、
既存presentation/anchor/singleton placementを共有する。白紙のaddressを発明しない。
**P2-2: 通常本と同じ制限付きの組立て**:

```text
既存source列挙 / page count + 構成record
 → pureなorder/hidden適用、identity/可視件数/ページ指定の解決（寸法不要）
 → 既存CONTAINER_ENTRY_LIMIT / ContainerEntryBudgetによる返却列
 → 既存寸法/回転取得（返却itemsを渡す）
 → 共通Remote builder / presentation（制限後nav + その範囲の指定 + 既存complete判定）
 → entries/PageGroup/既存truncated情報
```

- 対象の本の並べ替え/hiddenは制限より先に適用する。返却をraw順で先に切ってから並べ替えたり、
  hiddenを後から除いて返却数を不必要に減らしたりしない。子containerの既存位置/本scope境界は維持。
  元manifest/PageRefと可視件数は既存列挙情報/文書page countからpureに解決するだけで、全ページのheader/寸法を準備しない。
- 制限後itemsだけを現行cached_landscape_flags_with_read/catalog_free_dimsへ渡す。
  カタログの既存一括読取・既存PDF/EPUB page-size APIを変更せず、省略ページへ新たなheader読取/寸法解決を足さない。
  Singleの寸法取得省略も維持。新worker/第二group cache/隣相方の追加取得経路を増やさない。
- completeは構成適用後の可視本全体を返せたかと通常の資格条件で判定。hiddenだけならincompleteではない。
  件数/byte切詰めならfalse。自動after-cover/last、endpoint、final_coverは通常本同様に抑止する。
  effective設定値がOnでも切詰め中に適用されない点を、保存値の変更や設定消失と扱わない。
- 明示Alone/BlankOppositeは返却範囲に含むページの指定として渡し、complete=falseだけを理由に取り除かない。
  応答外の指定ページを呼び寄せず、既存自動規則もその境界を越えて全本から再計算しない。
  明示白紙は形成理由をresolverまで渡し常に白紙を優先。制限列の末尾へ表紙を追加しない。
- 切詰め境界は通常本と同じに扱う。全本Pairの元slot保存や省略相方addressを追加する契約はBでは作らず、
  制限後の列で組んだ最後のSingleも現行どおり閲覧可能にする。groupを削って表示可能ページを減らさない。

**PCとの一致の範囲**: 取得列・実効規則・complete・寸法/回転・modeが同じなら共通builderの結果は一致する。
制限により入力/completeが違う場合、構成本も通常本と同じくPCとPair membership/側/単独境界がずれ得る。
これは[延期済み§1.368](next-release-backlog.md)と同じ既知制約。Bで別経路を作らず§1.368の解決時に一緒に直す。
仕様・受入・manualにこの限界を明記し、全応答でPC parityを要求しない。

構成editorの全体編集は今回はPCのみ (Q6決定)。Remoteは構成で読み、非表示を拒否する。
方向/表紙/一ページずらしと構成record変更はcoreで拒否。
端配置/強制単独/末尾表紙は既存本別preference writerで変更し、core共通builder/presentation入力を再取得（§9.1）。
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
現treeでは`remote_ipc/ui.rs::poll_remote_session`（1194）にcomposition_editor_acquire_readyが接続済み。
**mount非依存editor ownerが終端 + 送信済save ticketが終端** を既存acquire barrierのFinish条件として維持する。
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
editorをPC限定にしRemoteには構成record writeを持たせないので、新data_version watcherは要らない。
Remote構成小編集のcommit/reply/revision通知と、それを理由にしたcontainer/group cache失効は撤去する。
mode-only変更、force_single/端末orientationは構成revisionを増やさず端末ごとのprojection keyへ載せる。
mode変更に必要な既存remote generation/cache失効は閲覧ownerから行い、撤回した構成write通知とは分ける。
構成なし本および構成本で許可する自動規則変更の既存remote generation/invalidation通知は維持。
規則変更で構成revisionを増やさずgroups cacheは実効規則/可視列/指定/寸法tokenで失効。
現treeのIPCはv70。payload/資格を変える実装時は統合時の最新+1、Web asset tokenとstrict handshake/testを一緒に更新する。

## 11. トリム・変換書庫・出力の契約

units依存の接点は§2.1。composed_blank_spread_sideのResolvedUnit探索をやめ、
共通builderのaccepted presentationの画面側とPageOverride由来をtrimへ渡す。
order/hidden/寸法/自動設定でslotが変われば新しい側を使う。trim record、既存raw/processed/edit/AIのsource所有は維持。
blankをsourceへ変換しない。

### 11.1 view-trim spread_separate

本別 `spread_left/right` は画面の側への設定として保持し、ページidentityへ焼き込まない。
pairを変えてページが反対側へ移れば、その新しい側のmarginが掛かる。RTL変更も同じ。
page overrideは従来どおりページ固有で、本設定との優先順位は `view_trim::stored_view_trim_bbox`へ集約する。
構成保存でmarginをswap/average/resetしない。crop/mask/annotation/edit空間も変更しない。

Single+blank/片側配置も**spread閲覧中のslot側**で本別spread marginを選ぶ (Q8決定)。
普通のSingle閲覧/編集pivotはsingle margin。blankにtrim/auto-trim/partner decodeを要求しない。
Autoの二枚上下harmonizeは実partnerがあるPair/末尾に表紙を添えたpresentationに通常本と同じ規則で適用。
末尾の明示BlankOppositeは指定実ページ側+白紙を維持し、表紙source/trim/partner需要と二枚harmonizeを発生させない。
自動強制単独の白紙を表紙へ置換する場合は従来どおり実ページ側を保持。添付表紙のsource/trimは元表紙の既存経路。
両者はaccepted unitの形成理由/presentationで区別し、白いcanvasの見た目だけで推測しない。
Navigationは一枚のままで、補助をpage-edit pivotや出力targetへ増やさない。
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
- direction/cover初期化と実効自動規則は現行cache側spread_container_keyを既存resolverで読む。
  構成保存先は元root、新recordへcacheキー/自動組をコピーしない。
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

製本された出力は可視実ページを連番へ焼き込む。構成DB/白紙/派生組を製本先へコピーしない。
これが「製本は構成作成対象外」と両立する。export順は構成順を採用 (元順も許容という要件の上位側)。
一枚書き出し/見開き合成/外部ツールは既存Navigation roleだけを使い、white canvasを画像に追加しない。
画面captureは見えている白い面を含む既存画面領域の意味を維持する。
metadata bundleは可視一覧選択からの画像exportとは別で、**非表示参照・Missing参照・単独/白紙指定を含む構成record全体**を運ぶ。
二つの「bundle」(メタ情報転送と製本用画像集合) を同じ処理にしない。

## 12. 消した状態の組み合わせと採否理由（B）

| 消した組み合わせ | 方策と理由 |
| --- | --- |
| 保存Pair/Single + order/hidden + landscape/page_alone | recordは実列の指定だけ、組は通常builder一つ。寸法/回転/設定変化も同入力 |
| hidden解除用の旧Pair/元slot | 参照はorder保持、可視列を繰り上げ。再表示は現在規則で再生成 |
| 新tail中央Single + 既存組凍結 | 新tailも実列末尾へ足し自動組。読取DB writeなし |
| 左page移動 + 右page/組移動 + 遠隔Pair | 順序ownerは左だけ、右はPageRefの排他的指定 |
| 単独bool + placement Option + blank bool + cover PageRef | PageOverride enum一つ、coverはbool/先頭role、独立白紙/手動補助なし。自動添付表紙は派生presentation |
| 通常builder + Composed専用 + editor/Remote独自組替え | 共通pure builder/presentation。構成は列/指定adapterだけ |
| 組変更candidate + draft + 全組差分承認/rebuild確認 | 即draft適用/右結果、一操作一undo。preview/承認phase撤去 |
| 構成 + session shift + saved shift | ページ指定が境界owner、shift休眠、解除時のみEの現在規則 |
| 全pairing禁止 + 自動規則追従の矛盾 | record編集はeditor、自動端/単独は既存設定owner、modeは閲覧owner。資格をactionで判定 |
| 組ごとの手動補助指定 + final_cover自動追加 | 手動補助は保存しない。明示BlankOppositeの由来が白紙を優先、それ以外は通常final_cover条件で派生 |
| 明示白紙と自動白紙の同一placement化 | 単独形成理由に由来を保持しresolverへ。明示Blankは添付禁止、自動白紙は既存置換を維持。別boolなし |
| Remote切詰め + 全本組生成/省略ページ寸法取得 | order/hidden後に既存範囲制限、返却itemsだけ既存寸法/組生成へ。PCとの差は§1.368へ分離 |
| mode/orientation/Splitのrecord変更 | 同実列のview projection。mode-only/端末強制は非構成write |
| 編集/保存中のopen/history/F12/sibling操作 | 既存process modal。常時モデルレス/割込みdraft退避/再開なし |
| editor/Remoteの別gate/二重write | 既存session境界・全窓close・ticket一回終端。後のRemote優先 |
| 保存中viewer rows変更 + draft順 | draft→transaction→既存reload。新列/旧組混在なし |
| sibling reload順がresumeを奪う | 保存owner一回intent、窓内anchorは各旧順。refresh/import/退避復帰は履歴/resume writeなし |
| ★固定/F12/parked Bookの旧構成復活 | recipe/tokenによる復帰adoption。関係Bookだけreload、結果は保存refreshなし |
| cached texture/prefetchとHidden許可 | 新提示だけworker照合、連結も同demand owner。Q16全列lookup/marker/永久skipなし |
| logical source/physical cache identity混合 | 既存typed snapshot、構成は元root、個別編集は現行key、EPUB元本+番号。reflow救済なし |
| source構成/cache spread-trimのorigin競合 | 専用family/coverage/keep-clear-replace維持。新record全体、受入先規則で組生成 |
| Folder構成のsidecar v2/receipt復活 | G4F対象外。既存v1/folder閲覧/ZIP変換維持 |
| 未公開units migration/互換builder | 新record一つ。released DB/構成sectionなし旧bundleは維持 |
| I/O/DB異常/強制終了の復旧網 | 既存一transaction/旧record・原本保持/ログ+通知。retry/永続draft/多段rollbackなし |

検討して採用しなかった簡略化:

- 保存反映で全窓close/設定reset: 通常保存に不要な退行。関係Bookの既存reload/identity着地を維持。
- 環境設定を全てrecordへコピー: 通常規則への追従を失い決定2に反する。
- direction/coverの第二閲覧値: 保存指定と表示が食い違う。editor上部へ集約。
- 構成本shiftの追加: 第二境界ownerを増やす。旧制限を残し通常本released shiftは維持。
- 結果全件hidden除外/再pair: Q16の候補だけ照合/結果順/保存refreshなしを失う。Book繰り上げと区別。
- 末尾表紙廃止/構成本だけの一律不適用: 訂正後の利用者再決定に反する。明示白紙だけは設計担当の優先決定に従い添付しない。
- Remote全本先行組生成/省略相方の追加取得: 省略ページの寸法I/O/別経路を増やし延期§1.368を先取りする。通常本と同じ制限経路を維持。
- 連結/Split/Remoteの一時無効化やPDF全件完了待ち: 既存機能を減らすため不採用。

組立て統一とmodalを先に採用し、rollback/supersession/resumeを網羅する新しい機械を足さない。

## 13. 段階・担当範囲・受入テスト

### B1: 設計改訂のみ（採択済み）

成果物は本書とhtdocs/mimageviewer/manual/books.html。製品コード/DB/実行時設定は変更しない。
設計を書き終えて停止し別担当の独立設計レビューへ渡す。コミット/製品テスト/確認binary/UI起動は行わない。
独立設計レビューACCEPT、`4f3d160f3`までコミット済み。以下のB2は2026-10-13に利用者が開始を指示した。

### B2: 実装と受入検証

1. model/store/転送: 新record round-trip、重複/unsafe/上限拒否、旧units migrationなし。
   order/hidden/override改名、Missing/新tail、KeepDrive/scope/domain/内容ID/単一writer。
   旧bundle不在保持・新版空解除・coverage外保持・本行全体replace、受入先規則で再生成。
2. 共通builder: 通常本の空override oracleを維持。LTR/RTL×cover×0/1/2/3/4/5/多数×横長/回転/寸法後着×
   両endpoint×after-cover/last×complete/subset×指定優先順位をpure test。
   A B C D EでB非表示→[A,C][D,E]、再表示、Alone/Blank/解除、末尾追加、Missing復帰、実削除直後F5前。
   指定直前/直後/cover/after-cover/lastとの重複・連続白紙指定で実ページ各一occurrence。
   通常本final-coverは不変、構成本も全体On/Off×本別FollowGlobal/On/Off（exact/fallbackを含む）で同じ出力。
   末尾hidden/Missing/policy、先頭表紙hidden、Alone/BlankOpposite/自動強制単独/横長、最後Pair/0枚/1枚/coverなし/subsetを検査。
   末尾の明示BlankOppositeは全体/本別final_coverがOnでも白紙/実ページ側を保持、補助occurrence・source需要ゼロ。
   同じ画面側の自動ForcedLast等の白紙は既存条件で表紙へ置換。明示と自動が重なる場合は明示由来優先。
   明示解除（自動へ戻す）後は通常添付条件へ戻り、設定On/Offで明示由来が消えない。非末尾指定は不変。
   添付表紙はNavigationへ重複せず手動補助保存ゼロ。PC/editor/連結は同入力で一致、
   Remoteは同取得列/complete/規則/寸法で一致を確認し、切詰めPC差は§1.368既知制約として別fixtureに固定。
3. position/permission/trim/出力: PageIdentity/raw互換/ordinal、旧順の次→前→先頭→一覧、単一resume intent。
   page override/左右trim/白紙・通常本trim不変。末尾明示白紙は表紙partner需要/harmonizeなし、
   自動白紙の表紙置換は従来どおり。hidden target拒否/Q16遅延許可の回帰を弱めない。
   出力は可視実列順、白紙/補助sourceなし、metadataはhidden/Missing/overrideを運ぶ。
4. editor: 左だけorder/hidden、上部direction/cover、右だけ指定/badge、直接結果+undo/redo。
   組差分確認/pending_preview/rebuild/unit移動経路ゼロ。右にhiddenを出さず選択/scrollを維持。
   小画面/theme/IME/snapshot、固定操作行/着地説明/解除確認、modal/Remote取得/ticket/外部変更検証。
5. operations/Remote/multi-context: direction/cover/Spread shift拒否、mode-only/端/単独/final-cover preference許可。
   キー/gamepad/IPC/setterの資格一致。自動規則変更でrecord/revision不変、groups cache更新。
   fork/mount/close/別scope不変、parked/stashed Bookの復帰鮮度、兄弟完了順によらない共有resume。
   構成order/hiddenを適用後に既存件数/byte制限を掛け、返却itemsだけ既存寸法/組生成へ。
   cap直前/直後のhidden・並べ替え・明示指定、0件/完全応答/件数切詰め/長名byte切詰めでtotal/truncated/completeを検査。
   同じ可視列/制限/空overrideで通常本と同じgroups。切詰めafter-cover/last/endpoint/final_coverは非適用、明示指定は範囲内で維持。
   カタログ未登録の返却ZIPだけheader probe、省略entryの追加header probeゼロ。Singleは横長判定の寸法解決呼出しゼロ。
   既存PDF/EPUB寸法APIの呼出し条件/回数を増やさず、新しい全本寸法要求/隣相方取得/境界group削除なし。
   full [0],[1],[2,3],[4]とbounded4 [0],[1,2],[3]等の§1.368制約を保持して試験し、後続解決の構成あり/なし共通fixtureへ引き継ぐ。
   orientation/force_single非構成write。

後続実装者がnarrow→共有gate/fmt/glyph/snapshotを所有し有効証跡を独立reviewerへ渡す。
確認binary/承認済み隔離実機検証はAGENTS.mdの通常手順。B1の文書検査を製品検証として数えない。
実装時はarchitecture/display/virtual-folders/web-remote/keymap/metadata/spec/manual関連を現行化し、
detached経路変更前の構造合意/§11記録を守る。B1は他の正本/製品ページへ未実装変更を広げない。

#### B2実装の接点と前提確認（2026-10-13）

- `BookComposition`/DBはschema 2。direction/cover_enabled/page_order/hidden_pages/page_overridesを保存し、
  旧schema・unitsを変換しない。読めないDBは既存の扱いで保持する。metadata v11も新record一つを運ぶ。
- pure projectionは可視実列・指定・identityだけを解く。paged/連結/editor/Remoteは
  `build_spread_display_units_with_overrides`と既存presentation resolverを共有する。
  ExplicitAlone/ExplicitBlankOppositeの形成理由で自動白紙と区別する。
  寸法後着と回転は共通cacheのepochへ従い、休眠shiftは適用しない。
- エディターは直接draft操作とundo/redo。結果workerは未送信draftを最新へまとめ、古いgenerationを採用しない。
  更新中は保存を止め、順序・非表示・指定・undo/redoは続けられる。hiddenは更新待ちの補助表紙にも表示しない。
  右の行は画像・ラベル専用帯と、その隣の説明・印の実高の大きい方から決め、一回だけ親へallocateする。
  本文と固定操作欄の自然高を分離し、操作欄は実文字/ボタンの寸法と幅から1行/2行を決める。
  200%小画面は件数をタブに統合し、指定ボタン・左右選択・カード全体の実clip内到達も検査する。
  master `f5bdab499`の`tests/support/scroll_layout.rs`を同じ内容で導入し、実矩形・隣接pitch・仮想切替を1x/2xで検査する。
- 現在表示中のfacet外ページは、共通builderへ一枚だけ渡してpaged/連結に保持する。
  明示白紙側を保ち、他のfacet外ページ・自動補助は持ち込まない。通常本のfacet挙動は変更しない。
- Remoteはstrict IPC v71とaccepted `composition_revision`を使用。order/hidden適用後に既存件数/byte制限、
  返却範囲だけの寸法取得と共通組生成。明示白紙は既存presentation slotに画面側を渡し、
  Single/端末単ページ強制/Splitと通常本のsingleton trimは変更しない。§1.368の制限は保持する。
- §11.3の全冊専用raw列挙→製本入口は現treeにない。全選択は構成後items→実Sourceを既に使うため、その経路を受入検査で固定した。
  一方、結果列のselected出力のvisibility resolverは旧treeで未実装だった。
  B2で製本/一括出力workerの実Source受付へ共通候補判定を接続した。hiddenは除外・通知、結果選択順は保持する。
  出力runだけのsource/scope索引を一度準備し、各候補で最新snapshot/tokenを照合する。DB回復・全本寸法取得は追加しない。

#### B2全体gateで見つかった関連修正（2026-10-14）

- Appの既存スタック上限検査で110,264 bytesを観測。Preparing/Projectingが保持するProjectionOriginをBoxへ移し、
  109,832 bytesへ減らして既存110,000 bytes未満の条件を維持した。旧表示参照を共通getterへまとめ、
  scope/token/source/receiver/状態遷移・pendingのClone→Automaticは変えない。B2が増加を起こしたとは断定しない。
- キャプチャ入力の既存テストは一関数に独立した4個のAppを置いてdebug stackを溢れさせた。
  同じ入力列/時刻/期待値を4テストへ分け、各input mutexとcleanup、通常のstackサイズを維持した。製品入力処理は変更しない。
- 保存形式を使うコピー検査の旧JSONパスをpage_orderへ追従し、App contextのない一括出力fixtureには生成関数を用意した。
  Appの実出力factoryが付けるvisibility checkは維持する。
- 通常HUDのspread-shift画像8枚は既存d73d628411のitem_spacing.y=0修正に未追従だった
  （画像の旧更新は8bc7efc87）。製品HUDを変えず現行renderへ更新し、全8枚を目視した。

#### B2自動検証記録（2026-10-14、4f3d160f3からの未コミット差分）

| 検査 | 結果 / 証跡（target配下、再生成可能） |
| --- | --- |
| fmt / UI glyph / diff | `cargo fmt --all -- --check`、`python scripts/check_ui_glyphs.py`、`git diff --check`成功。b2-fmt.log / b2-glyph.log / b2-diff-check.log |
| normal / portable core check | `cargo check -p mimageviewer --bin mimageviewer-core`、同`--features portable`成功。b2-check.log / b2-check-portable.log |
| 構成・Remote・転送・出力の焦点検査 | 構成195、実Remote列挙3、metadata転送9、製本/一括出力4成功。b2-related-verification.log。後続の型保持変更は下のlib全件でも検証 |
| 行 / UI snapshots | 行5、構成snapshot8、通常UI snapshot174成功。b2-row-layout-tests.log / b2-snapshots-update.log / b2-ui-snapshot-final.log。更新したPNG22枚を目視（構成14、既存HUD追従8） |
| Remote JS | app-runtime 174成功。b2-remote-js.log。command-core 131成功はRemote担当の有効な実行結果を再利用 |
| 全体gate | `scripts/test-full.ps1 -SuppressCrashDialogs`を一回実行。初回compileの出力fixture追従漏れを修正し、同等workspace/vendor gateを個別に続行。workspaceのlib以外は成功、UI旧画像差分は上の174件で解消。b2-test-full.log / b2-workspace-gate.log |
| lib全件（修正後） | `cargo test -p mimageviewer --features pack-build-tools --lib`で12,635成功、0失敗、既存ignored 57。stack overflowで中断した分も全件再実行して確認。b2-lib-gate-final.log |
| vendor全件 | egui 25、egui-wgpu 11、eframe 18成功。b2-vendor-egui.log / b2-vendor-egui-wgpu.log / b2-vendor-eframe.log |
| 確認用build | `scripts/build-dev.ps1`成功、normal featuresのcore / Remote / EPUB workerをtarget/dev-runtimeへ配置。b2-build-dev.log。起動していない |
| 独立レビュー | ソースと自動gateの範囲でACCEPT、未解消所見なし。実機受入は別途 |

キャプチャ入力14件、Appサイズ検査、コピー検査の修正後成功はb2-capture-tests-final.log /
b2-stack-bound-repair.log / b2-copy-path-repair.logにも残す。旧test executableを一度誤指定した再現確認は
stale-binary logへ分離し、修正の成功証跡には数えない。

実機確認はアプリを起動せず利用者へ引き継ぐ。隔離data-dirは`G:\miv-iso\book\data`、
媒体は`G:\miv-iso\media\book_zip.zip` / `book_pdf.pdf`等。
順序/hiddenの組み直し、Alone/白紙/末尾表紙の優先、undo/redo、200%時の行/操作欄、
保存・再開位置/しおり/履歴・別窓、Remote、trim・転送・製本/出力を確認する。
旧unitsの隔離データがある場合は削除せず退避し、新しい空の同data-dirで新形式を検査する。
通常のAPPDATAは使わない。コミットは実施しない。

### 旧G段階の実装・採択・検証履歴

**以下から§14直前まで旧units方式の履歴。G受入リスト/将来案はBの現行仕様を上書きしない。**
残存コードと旧結果の由来を追えるよう保持する。新仕様で変わるfixtureは上のB受入へ置換し、
通常本/Q16/E/所有境界の既存回帰は再利用する。

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
metadata preview/import/exportとGeneric改名maintenanceの開始も、この同じ保存票の完了・採用を待つ。
保存workerがserviceへwriteを送る前に窓を閉じても、転送previewがmaintenanceを先取りしない。
転送は既存WaitingForWritersの待ち先を型で分け、UIで待機・DB/FS処理をせず、並列のpending ownerを作らない。
folderのsidecar restoreは構成maintenanceを使わないため、この待ちを追加せず従来の開始条件を維持する。

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

### G4F: 画像だけの本フォルダ (2026-10-12 利用者決定で切り離し・将来案)

**切り離し決定（2026-10-12 利用者）**: 以下は採択前の将来案であり、今回の実装・gateには含めない。
`6b6b259e6`と`ecd2eaf7e`のfolder adapter/editor/sidecar v2・専用tests/snapshotを撤去する。
共通pure model、BookScopeKey、A2/Q16、G4 owner/ticketは再設計しない。未リリースのためmigrationは不要。
単純な二コミットの撤去だけでは、G3で先取りしたImageFolder行のruntime消費が残るため、
通常/抽出/退避画像の提示候補、類似preview、RemoteのFile候補・binding受付・構成管理表示も対象外へ揃える。
型・pure resolver・store・明示転送のfolder値は将来用として保持し、既存DB行を削除・移行しない。
File画像は既存の候補なし/Allowed(None)を使い、画像閲覧・個別編集・Eのfolder見開き調整を妨げない。
Remoteのwire形状は変更しない。保護対象を持たない完全空のwrite admissionは構成snapshotへ依存せず、
対象がある書庫/PDF/EPUBのadmissionは従来どおり失敗時に書込を拒否する。
切り離し後のRemote資格はworkerの実体file/dir分類に揃える。PDF/EPUB/ZIPを外部操作で同名folderへ
置き換え、旧Document/Archive構成行が残っていてもbinding受付・構成管理表示へ使用しない。
UIはworkerで確定したadmissionを必須とし、同期FS確認や拡張子/保存行からの再推測を行わない。
test-onlyの未修飾入口も同じworker資格へ通す。旧行/tokenは保持し、実ファイルの構成保護は維持する。
実体fileの資格と保存値の適用可否は分離し、将来用ImageFolder値をfileの構成に使わない既存分岐も維持する。
書庫/PDF/EPUBにも必要なGeneric改名開始時の送信済保存票確認は残す。
Checking予約のfolder root除外・改名mirrorからcache採用までのfolder claimは、mirror自体と一緒に外す。
構成のないfolderの通常・定期・終了・tray flushはmasterと同じ既存v1 writerへ戻し、改名claimで省略しない。
画像だけのBook folderで既存editor入口を使った時は、editor/state/構成を作らず次の案内だけを表示する。
「画像だけのフォルダでは本の構成を作れません。ZIP に変換してから行ってください。」
既存の採用済ProjectionRecipeで判定し、追加走査や新しい状態は設けない。
次段「画像フォルダをZIPに変換する機能」は2026-10-12の利用者決定を記録した
[独立設計](image-folder-to-zip-plan.md)を実装前に独立レビューする。
同じ場所のStored ZIP、元folder保持、初回からの利用者データ複製、同名ZIP時の停止、
modalと同名folder/本の3択（2026-10-12利用者決定・案B、新既定「本を表示」）を扱う。
旧released boolはON（既定を含む）→「本を表示」、OFF→「両方表示」へ移す（2026-10-12利用者決定）。
「重要な変更点」で告知し、環境設定「同名ファイル」の「フォルダを表示」で元の見え方へ戻せることを案内する。
現時点でボタン・変換処理は追加しない。

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

Q1〜Q17は2026-10-10の履歴。B1と競合する箇所は下のQ18〜Q24（2026-10-13）が優先。旧回答は経緯として保持する。
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

### 2026-10-13 追記（今回の利用者決定、番号固定）

| # | 利用者の決定 | 置換する旧契約 |
| --- | --- | --- |
| Q18 保存する値と場所 | direction/cover on-off/order/hidden/ページ指定だけ、組は保存しない。上部に方向/表紙、左にorder/hidden、右に指定/印。離れたPair廃止、左で隣へ並べる | Q2/Q3/Q17・固定units/任意cover PageRef/二か所の移動 |
| Q19 自動組と設定 | 指定なしは通常見開き規則。横長単独、中央/片側・強制単独等の利用者設定を尊重。表紙はcover on-off | 初期規則凍結/landscapeを無視するPair |
| Q20 末尾の単ページへ表紙を添える（説明訂正後の決定、2026-10-13） | 構成あり本も通常本と同じ全体既定/既存本別final cover preferenceに従う。末尾明示白紙の優先は設計担当のQ25、Remote範囲制限はQ26に従う。組ごとの手動FrontCoverSupplementは保存しない | 旧Q20の「補助不要」とB1の構成本だけ不適用を取り消し。Q3の手動組補助は廃止 |
| Q21 組変更確認 | 操作ごとの全組確認不要。右へ直接結果、undo/redo。組以外の安全確認は要否/理由を設計で判断 | G4候補/組差分確認/rebuild確認 |
| Q22 非表示後の組 | 片側を隠せば残りを繰り上げて組み直す | hiddenでも元Pair/slotを保持 |
| Q23 保存形式 | §1.242・book_composition.db未公開、v4.6.0対象外。形式作り直し可/移行不要 | 試作schemaの互換処理は作らない |
| Q24 出荷と段階 | 次の版へ。B1は設計とmanualのみ、完了後停止/別担当が独立設計レビュー。コード変更/コミットなし | 2026-10-11「次々回」と旧G後続の進行 |

### 2026-10-13 独立レビューREVISEへの設計担当の決定

Q25/Q26は利用者の新しい回答ではなく、今回伝達された設計担当の決定/方針。利用者決定Q18〜Q24と区別する。

| # | 設計担当の決定 / 方針 | 反映先 |
| --- | --- | --- |
| Q25 末尾の明示白紙（P2-1） | 末尾に「反対側を白紙」を明示した場合は白紙優先、末尾表紙を添えない。自動強制単独の白紙置換は維持。形成理由をresolverまで渡して区別 | §4.2/§5.1/§8/§11.1/§13/manual |
| Q26 Remoteの範囲（P2-2） | 状態/経路を増やさず構成なし本と同じ取得制限・境界・寸法取得範囲。order/hidden後の列を制限して組み、新しい省略ページ寸法読取/全本寸法解決なし。PCとの差は延期§1.368の解決で一緒に直す | §2.2/§5.1/§10.2/§12/§13/manual |

設計担当が「表紙の隣の空き側へ写しを薄く出す」と誤説明した経緯があり、正しい末尾添付の説明後、利用者がQ20を決め直した。

**明示決定と設計判断の区別**: §9.1のshift/binding制限維持と自動端/単独許可、
白紙側Leading/Trailing、明示Alone中央、先頭可視ページを表紙とする扱い、Q16結果列の例外、
保存着地説明/解除確認を残す判断はB1で理由を付けた設計案。末尾表紙の通常規則適用は訂正後Q20の明示決定。
利用者が個別採択済みとは書かない。Q18〜Q24は今回の明示指示の記録。

独立レビューの主要論点は共通境界走査の優先順位/余りの配置、値ownerで分ける操作資格、
Q16結果列とBook繰り上げの区別、明示/自動白紙の形成理由と末尾表紙判定、Remote制限付き生成と§1.368の境界。
P2-1/P2-2の対応内容とコード根拠は§2.2。再レビューのACCEPTや製品検証を完了したとは扱わない。
追加ページ指定は不要と判断。必須の利用者質問はないが、
shift/direction/coverのeditor外変更も許す意図があれば実装前に確認する。
その場合、通常本のreleased挙動を削らず、owner変更と費用を示して相談する。
