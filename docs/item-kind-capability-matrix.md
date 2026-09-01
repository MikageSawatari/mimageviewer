# 機能 × `GridItem` 種別 capability matrix

## 調査条件

- 基礎調査対象は commit `a94b6b1b` のコードで、§4 の外部ツールだけ P2b-1 実装まで追補した。列はブリーフ指定どおり、
  `GridItem` の `Image` / `Video` / `Audio`、`Folder`、`ZipFile` / `PdfFile`、
  `ZipImage`、`PdfPage`、`Stack`、`ZipDir` をまとめた
  (`src/grid_item.rs:33-98`)。`ConvertibleArchive` と `SearchContainer` も enum にはあるが、
  今回指定された列の対象外なので追加していない (`src/grid_item.rs:48-51`,
  `src/grid_item.rs:103-113`)。
- **対応** / **拒否** / **コンテナへ寄せる** / **無反応** はブリーフの定義どおりである。
  **拒否**には、無効化、トースト、対象メニューを出さない、そもそもその surface の
  対象にしない、を含め、各セルに方法を記した。ブリーフが一列にまとめた
  `ZipFile` / `PdfFile` 内で結果が違うセルは、variant 名ごとに分類を明記した。
- 「検索」は実装上 3 系統で対象集合も保存先も異なるため、Ctrl+S、Ctrl+G、Ctrl+F に
  分けた (`src/name_bulk_indexer.rs:339-362`, `src/search_walker.rs:289-323`,
  `src/app/metadata_ops.rs:1388-1522`)。「コピー・移動」も、ファイルの Copy/Cut、外向き
  D&D、パス文字列コピーでは resolver と失敗時挙動が異なるため分けた
  (`src/grid_item.rs:270-318`)。タグは単一通常 UI と quick action / 混在一括で拒否方法が違い
  (`src/ui_main.rs:5629-5643`, `src/tag_ops.rs:133-148`, `src/tag_ops.rs:208-225`)、削除も
  右クリック、`Delete` key、checked で非対応 item の見え方が違うため分けた
  (`src/context_menu_model.rs:263-311`, `src/ui_dialogs/context_menu.rs:1532-1544`,
  `src/ui_dialogs/context_menu.rs:1897-1916`)。これがブリーフの行に対して追加した内訳である。
- 表の「グリッド」は本稿上の main-window surface、「ビューア」は fullscreen / detached の
  viewer surface を指す。タグ操作ではそれぞれ `ActionSurface::MainWindow` / `Viewer` に対応する
  (`src/tag_ops.rs:78-148`)。検索・スマートフォルダについては、結果集合を作る main-window 側の
  処理を判定対象とした。

## 1. レーティングとタグ

| 機能・入口 | 実ファイル (`Image` / `Video` / `Audio`) | `Folder` | コンテナファイル (`ZipFile` / `PdfFile`) | `ZipImage` | `PdfPage` | `Stack` | `ZipDir` | 保存先・対象ファイルの変更 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| レーティング ★ — グリッド (`src/app.rs:34094-34103`) | **対応**。3 種とも leaf rating 対象で、F1～F6 は選択対象への付与を呼ぶ (`src/grid_item.rs:152-165`, `src/app.rs:34094-34103`)。 | **対応**。container rating 対象 (`src/grid_item.rs:167-181`)。 | **対応**。両方とも container rating 対象 (`src/grid_item.rs:167-181`)。 | **対応**。ZIP entry の page key と kind `6` でページ単位に保存する (`src/app.rs:47605-47616`, `src/rating_db.rs:28-57`)。 | **対応**。PDF page key と kind `7` でページ単位に保存する (`src/app.rs:47605-47616`, `src/rating_db.rs:28-57`)。 | **無反応**。rating 対象から外れ、対象 0 件なら通知なしで return する (`src/grid_item.rs:208-214`, `src/app.rs:49283-49285`, `src/app.rs:49565-49573`)。 | **対応**。ZIP path と directory prefix の合成キーを持つ container rating 対象 (`src/grid_item.rs:167-181`, `src/app.rs:47623-47630`)。 | 正本は `rating.db` (`src/rating_db.rs:1-6`, `src/rating_db.rs:160-161`)。まず DB を更新し、設定 ON の非製本 `Image` だけ後述の XMP worker へ送る (`src/app.rs:48017-48063`)。 |
| レーティング ★ — ビューア (`src/ui_fullscreen.rs:20231-20247`) | **対応**。現在 item に直接 `set_rating` する (`src/ui_fullscreen.rs:20231-20247`)。 | **対応**。通常画像の「現在のコンテナ」行として別操作になる (`src/ui_metadata_panel.rs:328-341`, `src/ui_metadata_panel.rs:464-472`, `src/app.rs:48682-48749`)。 | **対応**。ZIP/PDF の親コンテナ行とページ行を別々に表示・実行する (`src/ui_metadata_panel.rs:328-341`, `src/ui_metadata_panel.rs:464-472`)。 | **対応**。親とは別のページ行・ページ key に付く (`src/ui_metadata_panel.rs:328-341`, `src/ui_metadata_panel.rs:464-472`, `src/app.rs:47605-47616`)。 | **対応**。親とは別のページ行・ページ key に付く (`src/ui_metadata_panel.rs:328-341`, `src/ui_metadata_panel.rs:464-472`, `src/app.rs:47605-47616`)。 | **拒否**。`Stack` のままビューア対象にせず、member `Image` の配列へ展開する (`src/filename_stack_ui.rs:595-660`)。 | **対応**。「現在のコンテナ」行から ZIP path + prefix の合成キーへ付く (`src/app.rs:48691-48724`)。 | グリッドと同じ `rating.db` (`src/rating_db.rs:160-161`)。XMP 対象判定も同じ `rating_xmp_target_for_idx` を通る (`src/app.rs:47669-47682`, `src/app.rs:48051-48063`)。 |
| タグ — グリッド・単一の通常メニュー / `T` (`src/ui_main.rs:5629-5643`) | **対応**。実パスを target にする (`src/tag_ops.rs:21-40`)。 | **対応**。folder path を target にする (`src/tag_ops.rs:21-40`)。 | **対応**。ZIP/PDF 自身の実パスを target にする (`src/tag_ops.rs:21-40`)。 | **拒否**。グリッドでは tag target がなく、上部メニューは無効、`T` は「タグ対象なし」トースト (`src/tag_ops.rs:21-40`, `src/ui_main.rs:5629-5643`, `src/ui_dialogs/tag_apply.rs:23-31`)。 | **拒否**。グリッドでは tag target がなく、メニュー無効 / `T` でトースト (`src/tag_ops.rs:21-40`, `src/ui_main.rs:5629-5643`, `src/ui_dialogs/tag_apply.rs:23-31`)。 | **拒否**。tag target がなく、通常メニュー無効 / `T` でトースト (`src/tag_ops.rs:21-40`, `src/ui_main.rs:5629-5643`, `src/ui_dialogs/tag_apply.rs:23-31`)。 | **拒否**。tag target がなく、通常メニュー無効 / `T` でトースト (`src/tag_ops.rs:21-40`, `src/ui_main.rs:5629-5643`, `src/ui_dialogs/tag_apply.rs:23-31`)。 | 正本は `tags.db` (`src/tags_db.rs:1-4`, `src/tags_db.rs:95-97`)。通常 worker は DB だけを更新し、メディア / XMP / Tantivy を書かない (`src/tag_write_worker.rs:1-5`, `src/tag_write_worker.rs:320-430`)。 |
| タグ — グリッドのピン留め quick action / 混在一括 (`src/tag_ops.rs:133-165`) | **対応**。解決された実パスを worker へ送る (`src/tag_ops.rs:133-165`)。 | **対応**。folder path を worker へ送る (`src/tag_ops.rs:21-40`, `src/tag_ops.rs:133-165`)。 | **対応**。コンテナ実パスを worker へ送る (`src/tag_ops.rs:21-40`, `src/tag_ops.rs:133-165`)。 | **無反応**。型 map が target を返さず、単独では target 0 件で silent return、実項目との混在時はこのページだけ黙って脱落する (`src/tag_ops.rs:21-40`, `src/tag_ops.rs:133-148`, `src/tag_ops.rs:208-225`, `src/app/gamepad_input.rs:5168-5186`)。 | **無反応**。`ZipImage` と同じく単独 silent return / 混在時 silent drop (`src/tag_ops.rs:21-40`, `src/tag_ops.rs:133-148`, `src/tag_ops.rs:208-225`, `src/app/gamepad_input.rs:5168-5186`)。 | **無反応**。tag target 0 件のまま quick action が呼ばれ、silent return する (`src/tag_ops.rs:21-40`, `src/tag_ops.rs:208-225`, `src/app/gamepad_input.rs:5168-5186`)。 | **無反応**。tag target 0 件のまま quick action が呼ばれ、silent return する (`src/tag_ops.rs:21-40`, `src/tag_ops.rs:208-225`, `src/app/gamepad_input.rs:5168-5186`)。 | 通常タグと同じ `tags.db` (`src/tags_db.rs:95-97`)。成功時だけ任意の sidecar mirror 処理へ進む (`src/tag_ops.rs:617-663`)。 |
| タグ — ビューア (`src/tag_ops.rs:78-148`) | **対応**。現在の実ファイルを target にする。通常画像では folder 行と image 行を分ける (`src/tag_ops.rs:21-40`, `src/ui_metadata_panel.rs:717-786`)。 | **対応**。通常画像の「このフォルダ」行から folder path へ付ける (`src/ui_metadata_panel.rs:717-786`)。 | **対応**。ZIP/PDF ページ表示では「この本 / この PDF」の一行が親実パスへ付く (`src/ui_metadata_panel.rs:701-716`, `src/ui_metadata_panel.rs:787-805`)。 | **コンテナへ寄せる**。viewer surface のときだけ `zip_path`（変換書庫なら元書庫）へ fallback する (`src/tag_ops.rs:29-67`)。 | **コンテナへ寄せる**。viewer surface のときだけ `pdf_path` へ fallback する (`src/tag_ops.rs:29-40`)。 | **拒否**。`Stack` 自体は tag target を持たず、ビューア前に member `Image` へ展開される (`src/tag_ops.rs:21-40`, `src/filename_stack_ui.rs:595-660`)。 | **拒否**。`ZipDir` 自身は tag target を持たない。子 `ZipImage` からは directory prefix でなく外側 ZIP へ寄る (`src/tag_ops.rs:21-40`)。 | 正本は `tags.db` (`src/tags_db.rs:95-97`)。ページ fallback 時も親 ZIP/PDF の DB key を更新するだけで、メディア本体は書き換えない (`src/tag_ops.rs:29-40`, `src/tag_write_worker.rs:1-5`)。 |

### 保存先の補足

以下の `rating.db` / `tags.db` はいずれも `<data_dir>` 直下を指す
(`src/rating_db.rs:160-161`, `src/tags_db.rs:95-97`)。

- レーティングは全対応種別をまず `rating.db` に保存する。`RatingItemKind` は
  `ZipImage = 6`、`PdfPage = 7`、`ZipDir = 8`、`Audio = 9` を持ち、`Stack` は持たない
  (`src/rating_db.rs:15-57`, `src/rating_db.rs:256-286`)。横断一覧も `ZipImage` / `PdfPage` /
  `ZipDir` を元の variant として復元する (`src/rating_view.rs:174-225`)。
- メディア本体へ ★ を埋め込むのは `write_rating_to_xmp == true` かつ
  `rating_xmp_target_for_idx` が返る場合だけである。対象は非製本の実 `Image` かつ JPEG
  （`.jpg` / `.jpeg` / `.jfif`）/ PNG / WebP (`src/app.rs:47669-47682`,
  `src/xmp_writer.rs:89-116`)。既定値は OFF (`src/settings.rs:6077`)。worker は
  `apply_rating` を呼び、同じ directory の一時ファイルを rename して画像本体を置換する
  (`src/rating_write_worker.rs:101-119`, `src/xmp_writer.rs:261-290`)。
- タグの optional mirror は親 folder の `mimageviewer.dat` で、正本ではない。実ファイルの
  `parent + lowercase filename` からだけ target を作り、`Folder` には作らない
  (`src/tag_write_worker.rs:21-27`, `src/tag_ops.rs:21-40`)。
  `tag_sidecar_backup_enabled` は既定 OFF (`src/settings.rs:6071`)、製本ページでは mirror target を
  明示的に落とす (`src/tag_ops.rs:114-121`)。これは別ファイルであり、メディア本体の変更ではない
  (`src/sidecar.rs:1-9`, `src/sidecar.rs:47-83`)。
- `xmp_writer::apply_tag_op` の実装は残る (`src/xmp_writer.rs:157-186`) が、リポジトリ全体の
  完全一致 `apply_tag_op(` 検索ではこの public 定義一件しかなく、caller はない。現行タグ
  worker 自身も「DB のみ」を契約にしている
  (`src/tag_write_worker.rs:1-5`)。

## 2. 削除

ここでは mIV 所有の「ゴミ箱へ移動（タグ・評価も整理）」を判定する。後から併記される
Windows Shell menu は別所有である。

| 機能・入口 | 実ファイル (`Image` / `Video` / `Audio`) | `Folder` | コンテナファイル (`ZipFile` / `PdfFile`) | `ZipImage` | `PdfPage` | `Stack` | `ZipDir` | 保存先・対象ファイルの変更 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| mIV 削除 — グリッド・単一右クリック | **対応**。real item として削除行を出す (`src/context_menu_model.rs:97-107`, `src/context_menu_model.rs:476-487`)。 | **対応**。`drag_source_path` を持つ real item (`src/grid_item.rs:288-307`, `src/context_menu_model.rs:476-487`)。 | **対応**。両方とも real item として削除行を出す (`src/context_menu_model.rs:97-107`, `src/context_menu_model.rs:476-487`)。 | **拒否**。単一 item の削除行を出さない (`src/context_menu_model.rs:140-142`, `src/context_menu_model.rs:476-487`)。 | **拒否**。単一 item の削除行を出さない (`src/context_menu_model.rs:140-142`, `src/context_menu_model.rs:476-487`)。 | **拒否**。単一 item の削除行を出さない (`src/context_menu_model.rs:140-142`, `src/context_menu_model.rs:476-487`)。 | **拒否**。単一 item の削除行を出さない (`src/context_menu_model.rs:140-142`, `src/context_menu_model.rs:476-487`)。 | 成功した実パスを Shell がゴミ箱へ移す。ゴミ箱不可等では警告後に完全削除になり得る (`src/delete_worker.rs:475-493`)。成功分だけ mIV の path-keyed metadata を hard purge する (`src/delete_worker.rs:250-309`, `src/rename_key_migration.rs:778-834`)。 |
| mIV 削除 — グリッド・単一 `Delete` key | **対応**。`drag_source_path` を確認画面へ渡す (`src/ui_dialogs/context_menu.rs:1873-1918`)。 | **対応**。folder の `drag_source_path` を確認画面へ渡す (`src/grid_item.rs:288-307`, `src/ui_dialogs/context_menu.rs:1907-1918`)。 | **対応**。実コンテナ path を確認画面へ渡す (`src/grid_item.rs:288-307`, `src/ui_dialogs/context_menu.rs:1907-1918`)。 | **拒否**。種別ごとの理由をトーストで返す (2026-08-30 修正、`GridItem::file_operation_refusal`)。 | **拒否**。同上。 | **拒否**。同上 (スタック向けの文言)。 | **拒否**。同上 (書庫内フォルダ向けの文言)。 | 対応セルだけ上と同じ Shell 削除 + 成功 path の hard purge (`src/delete_worker.rs:250-309`, `src/delete_worker.rs:475-493`)。無反応セルは変更なし (`src/ui_dialogs/context_menu.rs:1907-1916`)。 |
| mIV 削除 — グリッド・チェック選択 | **対応**。全 checked item が実 path なら削除する (`src/ui_dialogs/context_menu.rs:1156-1164`, `src/ui_dialogs/context_menu.rs:1532-1545`)。 | **拒否**。`Folder` は checkbox 対象外 (`src/grid_item.rs:310-318`)。 | **対応**。両方とも checkable かつ実 path を持つ (`src/grid_item.rs:270-285`, `src/grid_item.rs:310-318`)。 | **拒否**。checkable なので削除行は出るが、実 path 数不一致を検出し「ページは削除できません」トーストで混在を含め全体拒否 (`src/context_menu_model.rs:263-311`, `src/ui_dialogs/context_menu.rs:293-296`, `src/ui_dialogs/context_menu.rs:1532-1544`)。 | **拒否**。`ZipImage` と同じく、実行時トーストで全体拒否 (`src/context_menu_model.rs:263-311`, `src/ui_dialogs/context_menu.rs:293-296`, `src/ui_dialogs/context_menu.rs:1532-1544`)。 | **拒否**。checkbox 対象外 (`src/grid_item.rs:310-318`)。 | **拒否**。checkbox 対象外 (`src/grid_item.rs:310-318`)。 | 対応セルだけ成功 path を削除し metadata purge。仮想混在は削除前に全体拒否され変更なし (`src/ui_dialogs/context_menu.rs:1532-1544`, `src/delete_worker.rs:250-309`)。 |
| mIV 削除 — ビューア | **拒否**。mIV 削除行は `surface == Grid` のときだけ出す (`src/context_menu_model.rs:476-487`)。 | **拒否**。同条件に加え、Folder 自体を viewer leaf にしない (`src/context_menu_model.rs:476-487`, `src/ui_main.rs:13139-13184`)。 | **拒否**。同条件に加え、コンテナ自身を viewer leaf にしない (`src/context_menu_model.rs:476-487`, `src/ui_main.rs:13139-13184`)。 | **拒否**。viewer では mIV 削除行を出さない (`src/context_menu_model.rs:476-487`)。 | **拒否**。viewer では mIV 削除行を出さない (`src/context_menu_model.rs:476-487`)。 | **拒否**。viewer 前に member images へ展開し、削除行も出さない (`src/filename_stack_ui.rs:595-660`, `src/context_menu_model.rs:476-487`)。 | **拒否**。viewer leaf でなく、削除行も出さない (`src/context_menu_model.rs:476-487`, `src/ui_main.rs:13139-13184`)。 | mIV 削除は走らないため file / DB とも変更なし (`src/context_menu_model.rs:476-487`)。 |

完全削除は独立コマンドではない。確認前に recycle 可否・容量を調べ、完全削除になりそうなら
文言を切り替える (`src/ui_dialogs/context_menu.rs:430-450`,
`src/ui_dialogs/context_menu.rs:504-528`)。worker は `FOFX_RECYCLEONDELETE` と
`FOF_WANTNUKEWARNING` を同時指定するため、最終判断は Windows Shell が行う
(`src/delete_worker.rs:475-493`)。

実 path には Windows `IContextMenu` も委譲される (`src/native_context_menu.rs:415-451`,
`src/native_context_menu.rs:1027-1048`)。そこで OS / shell extension が「削除」を出すかは
mIV コードでは固定できないため **要調査（実行環境依存）**。その Shell command は mIV の
delete worker を通らない (`src/native_context_menu.rs:561-605`,
`src/ui_dialogs/context_menu.rs:1077-1082`) ので、表示された場合も上表の metadata hard purge は
保証されない。

## 3. コピー・移動

Copy/Cut → Paste の表は、mIV が持つ OLE clipboard shortcut / paste verb を判定する。
実 path に併記され得る Windows Shell menu は OS 所有で、末尾に別記する
(`src/app.rs:36575-36647`, `src/native_context_menu.rs:1027-1048`)。

| 機能・入口 | 実ファイル (`Image` / `Video` / `Audio`) | `Folder` | コンテナファイル (`ZipFile` / `PdfFile`) | `ZipImage` | `PdfPage` | `Stack` | `ZipDir` | 保存先・対象ファイルの変更 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| ファイル Copy/Cut → Paste — グリッド | **対応**。`drag_source_path` を OLE clipboard へ置く (`src/grid_item.rs:288-307`, `src/app.rs:36426-36448`)。 | **対応**。Folder も `drag_source_path` を持つ (`src/grid_item.rs:288-307`)。 | **対応**。ZIP/PDF 実 path を渡す (`src/grid_item.rs:288-307`)。 | **拒否**。単独・checked・混在とも path 収集を失敗させ、トーストで全体拒否 (`src/app.rs:36426-36461`)。 | **拒否**。`ZipImage` と同じくトーストで全体拒否 (`src/app.rs:36426-36461`)。 | **拒否**。`UncopyableItem` としてトーストで拒否（文言は ZIP/PDF ページ用） (`src/app.rs:36438-36461`)。 | **拒否**。`UncopyableItem` としてトーストで拒否（文言は ZIP/PDF ページ用） (`src/app.rs:36438-36461`)。 | Copy は OLE clipboard に COPY、Cut は MOVE の preferred effect を設定する (`src/native_context_menu.rs:27-37`, `src/native_context_menu.rs:630-689`)。Paste 後の destination / 衝突処理は Shell 所有 (`src/app.rs:36620-36647`)。mIV はメディア内容を書き換えない。 |
| ファイル Copy/Cut → Paste — ビューア | **拒否**。mIV の file-copy shortcut handler は viewer 中 block され、mIV 項目も持たない (`src/app.rs:36411-36424`, `src/app.rs:36575-36580`)。 | **拒否**。viewer leaf でなく、同 handler の対象外 (`src/app.rs:36411-36424`, `src/ui_main.rs:13139-13184`)。 | **拒否**。viewer leaf でなく、同 handler の対象外 (`src/app.rs:36411-36424`, `src/ui_main.rs:13139-13184`)。 | **拒否**。実 filesystem path がなく、native Shell target も作れない (`src/grid_item.rs:288-307`, `src/ui_dialogs/context_menu.rs:1136-1155`)。 | **拒否**。実 filesystem path がなく、native Shell target も作れない (`src/grid_item.rs:288-307`, `src/ui_dialogs/context_menu.rs:1136-1155`)。 | **拒否**。Stack のまま viewer target にならず、file operation path もない (`src/grid_item.rs:288-307`, `src/filename_stack_ui.rs:595-660`)。 | **拒否**。viewer leaf でなく、file operation path もない (`src/grid_item.rs:288-307`, `src/ui_main.rs:13139-13184`)。 | mIV の OLE clipboard / Paste 経路は走らない (`src/app.rs:36411-36424`)。実 leaf の Windows Shell menu に Copy/Cut が出るかは OS 委譲であり、上表の mIV 機能には含めない (`src/native_context_menu.rs:1027-1048`)。 |
| 外向き file D&D — グリッド・単一 | **対応**。実 path を `SHDoDragDrop` に COPY effect で渡す (`src/ui_main.rs:1691-1744`, `src/file_drag.rs:160-199`)。 | **対応**。Folder の `drag_source_path` を渡す (`src/grid_item.rs:288-307`, `src/ui_main.rs:1736-1744`)。 | **対応**。ZIP/PDF 実 path を渡す (`src/grid_item.rs:288-307`, `src/ui_main.rs:1736-1744`)。 | **拒否**。種別ごとの理由をトーストで返す (2026-08-30 修正、`GridItem::file_operation_refusal`)。 | **拒否**。同上。 | **拒否**。同上 (スタック向けの文言)。 | **拒否**。同上 (書庫内フォルダ向けの文言)。 | destination Shell へ COPY だけを要求する。元 path / mIV DB は変更しない (`src/file_drag.rs:160-199`)。 |
| 外向き file D&D — グリッド・checked（実項目だけ / 仮想 page だけ） (`src/ui_main.rs:1691-1735`) | **対応**。checked が実項目だけなら実 path 全件を drag する (`src/ui_main.rs:1691-1735`)。 | **拒否**。Folder は checkbox 対象外 (`src/grid_item.rs:310-318`)。 | **対応**。checked が実項目だけなら ZIP/PDF path 全件を drag する (`src/grid_item.rs:270-318`, `src/ui_main.rs:1691-1735`)。 | **拒否**。仮想 page だけの checked は事前トーストを出して drag しない (`src/ui_main.rs:1720-1723`)。 | **拒否**。仮想 page だけの checked は事前トーストを出して drag しない (`src/ui_main.rs:1720-1723`)。 | **拒否**。checkbox 対象外 (`src/grid_item.rs:310-318`)。 | **拒否**。checkbox 対象外 (`src/grid_item.rs:310-318`)。 | 実項目だけなら destination Shell へ COPY。仮想 page だけなら file / DB とも変更なし (`src/ui_main.rs:1691-1735`, `src/file_drag.rs:160-199`)。 |
| 外向き file D&D — グリッド・実 / 仮想 page 混在 checked (`src/ui_main.rs:1705-1735`) | **対応**。実 path は drag 対象に残り、部分実行される (`src/ui_main.rs:1705-1735`)。 | **拒否**。Folder は checkbox 対象外 (`src/grid_item.rs:310-318`)。 | **対応**。ZIP/PDF の実 path は drag 対象に残り、部分実行される (`src/ui_main.rs:1705-1735`)。 | **拒否**。この page は対象から除外されるが、実 path の COPY は部分実行し、完了後にトーストを出す (`src/ui_main.rs:1705-1735`)。 | **拒否**。この page は対象から除外されるが、実 path の COPY は部分実行し、完了後にトーストを出す (`src/ui_main.rs:1705-1735`)。 | **拒否**。checkbox 対象外 (`src/grid_item.rs:310-318`)。 | **拒否**。checkbox 対象外 (`src/grid_item.rs:310-318`)。 | 残った実 path だけ destination Shell へ COPY。除外 page と元 path / DB は変更しない (`src/ui_main.rs:1705-1735`, `src/file_drag.rs:160-199`)。 |
| 外向き file D&D — ビューア | **拒否**。drag start は grid cell response にだけ接続される (`src/ui_main.rs:13319-13335`)。 | **拒否**。grid cell drag 入口がなく viewer leaf でもない (`src/ui_main.rs:13319-13335`)。 | **拒否**。grid cell drag 入口がなく viewer leaf でもない (`src/ui_main.rs:13319-13335`)。 | **拒否**。grid cell drag 入口がない (`src/ui_main.rs:13319-13335`)。 | **拒否**。grid cell drag 入口がない (`src/ui_main.rs:13319-13335`)。 | **拒否**。grid cell drag 入口がない (`src/ui_main.rs:13319-13335`)。 | **拒否**。grid cell drag 入口がない (`src/ui_main.rs:13319-13335`)。 | D&D 処理が起動しないため変更なし (`src/ui_main.rs:13319-13335`)。 |
| パス文字列コピー — グリッド・単一 (`src/context_menu_model.rs:328-368`) | **対応**。実 path を clipboard text にする (`src/ui_dialogs/context_menu.rs:475-501`, `src/ui_dialogs/context_menu.rs:1366-1389`)。 | **対応**。folder の実 path を text にする (`src/ui_dialogs/context_menu.rs:475-501`)。 | **対応**。コンテナの実 path を text にする (`src/ui_dialogs/context_menu.rs:475-501`)。 | **対応**。filesystem path ではなく `zip_path:entry` の合成表示文字列 (`src/ui_dialogs/context_menu.rs:489-497`)。 | **対応**。filesystem path ではなく `pdf_path:Page N` の合成表示文字列 (`src/ui_dialogs/context_menu.rs:498-500`)。 | **対応**。「代表画像のパス」をコピーする (`src/context_menu_model.rs:328-343`, `src/ui_dialogs/context_menu.rs:485-488`)。 | **対応**。filesystem path ではなく `zip_path:prefix` の合成表示文字列 (`src/ui_dialogs/context_menu.rs:489-497`)。 | OS clipboard の text のみ。file / DB は変更しない (`src/ui_dialogs/context_menu.rs:471-473`, `src/ui_dialogs/context_menu.rs:1388-1389`)。 |
| パス文字列コピー — ビューア・単一 (`src/ui_dialogs/context_menu.rs:1610-1700`) | **対応**。viewer leaf の実 path を clipboard text にする (`src/ui_dialogs/context_menu.rs:475-501`, `src/ui_dialogs/context_menu.rs:1610-1700`)。 | **拒否**。Folder variant 自体を viewer leaf にしない (`src/ui_main.rs:13139-13184`)。 | **拒否**。コンテナ自身でなく子 page が viewer target になる (`src/ui_main.rs:13139-13184`, `src/ui_main.rs:13186-13220`)。 | **対応**。`zip_path:entry` の合成表示文字列をコピーする (`src/ui_dialogs/context_menu.rs:489-497`, `src/ui_dialogs/context_menu.rs:1610-1700`)。 | **対応**。`pdf_path:Page N` の合成表示文字列をコピーする (`src/ui_dialogs/context_menu.rs:498-500`, `src/ui_dialogs/context_menu.rs:1610-1700`)。 | **拒否**。Stack 自体は viewer 前に member Image へ展開される (`src/filename_stack_ui.rs:595-660`)。 | **拒否**。ZipDir variant 自体を viewer leaf にしない (`src/ui_main.rs:13139-13184`)。 | 対応時だけ OS clipboard text を更新する。拒否セルでは file / DB とも変更なし (`src/ui_dialogs/context_menu.rs:471-473`, `src/ui_dialogs/context_menu.rs:1388-1389`)。 |
| パス文字列コピー — グリッド・checked (`src/ui_dialogs/context_menu.rs:1366-1389`) | **対応**。実 path を改行区切りでコピー (`src/ui_dialogs/context_menu.rs:1366-1389`)。 | **拒否**。Folder は checkbox 対象外 (`src/grid_item.rs:310-318`)。 | **対応**。実 path を改行区切りでコピー (`src/grid_item.rs:270-318`, `src/ui_dialogs/context_menu.rs:1378-1389`)。 | **拒否**。menu は出るが、仮想を一件でも含むとトーストで全体拒否し何もコピーしない (`src/context_menu_model.rs:263-270`, `src/ui_dialogs/context_menu.rs:1366-1377`)。 | **拒否**。`ZipImage` と同じくトーストで全体拒否 (`src/context_menu_model.rs:263-270`, `src/ui_dialogs/context_menu.rs:1366-1377`)。 | **拒否**。checkbox 対象外 (`src/grid_item.rs:310-318`)。 | **拒否**。checkbox 対象外 (`src/grid_item.rs:310-318`)。 | 対応時だけ OS clipboard text を更新する。拒否時は clipboard / file / DB とも変更しない (`src/ui_dialogs/context_menu.rs:1366-1389`)。 |

実 path の viewer context menu には Windows `IContextMenu` も委譲される。その menu に
Copy/Cut が出るかは mIV が固定しないため **要調査（実行環境依存）**
(`src/ui_dialogs/context_menu.rs:1136-1155`, `src/native_context_menu.rs:1027-1048`)。
`ZipImage` / `PdfPage` は実 path がないので、この native menu 自体を構築しない
(`src/grid_item.rs:288-307`, `src/ui_dialogs/context_menu.rs:1136-1155`)。

## 4. 外部ツール起動 (`ExternalTool`)

| 機能・入口 | 実ファイル (`Image` / `Video` / `Audio`) | `Folder` | コンテナファイル (`ZipFile` / `PdfFile`) | `ZipImage` | `PdfPage` | `Stack` | `ZipDir` | 保存先・対象ファイルの変更 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| ExternalTool — グリッド | **対応**。右クリックは checked があれば checked 全件、無ければ右クリック項目、キーは checked 優先・無ければ selected を対象にする。クリック項目を先頭、残りを `current_grid_order()` 順に保ち、`Single` / `Each` / `Batch` で渡す。 | **対応 (コンテナー入口) / 拒否 (ページ入口)**。フォルダー項目の右クリックと物理フォルダー背景はフォルダー 1 件を渡す。通常のページ対象では `Unsupported`。 | **対応**。コンテナー項目の右クリックは checked を広げず、ZIP/PDF 自身 1 件を渡す。ページ入口で選んだ場合も実ファイルとして選択ポリシーを適用する。 | **拒否 (P3 まで)**。単独では viewing tool を非表示、editing tool を理由付き disabled にする。実ファイルとの混在時は選択全体を起動前に拒否し、部分起動しない。 | **拒否 (P3 まで)**。`ZipImage` と同じ仮想対象として選択全体を拒否する。 | **拒否 (P3 まで)**。仮想対象として選択全体を拒否する。 | **拒否 (P3 まで)**。仮想対象として選択全体を拒否する。 | tool 定義は `settings.db` の `external_tools` table。mIV は実 path を process / association / OS default へ渡すだけで対象 file を変更しない。外部 process が行う変更は mIV 管理外。 |
| ExternalTool — ビューア | **対応**。checked を無視して viewer の現在ページ 1 件を共通 resolver へ渡す。見開き中も P2a では 1 件のままで、`SpreadPolicy` は P4 まで適用しない。ただし spawn 成功前でも viewer close を予約する。 | **拒否**。Folder variant 自体を viewer leaf / tool target にしない。 | **拒否**。コンテナ自身でなく子ページが viewer target になるため、親へ fallback しない。 | **拒否 (P3 まで)**。グリッドと同じ仮想対象判定。 | **拒否 (P3 まで)**。グリッドと同じ仮想対象判定。 | **拒否**。Stack 自体は viewer 前に member Image へ展開される。 | **拒否**。ZipDir variant 自体を viewer leaf / tool target にしない。 | グリッドと同じ `settings.db` の tool 定義を読む。mIV は file を書き換えず、外部 process の動作は管理外。 |

`SelectionPolicy::Single` は上記対象集合の先頭 1 件、`Each` は 1 件ずつ、`Batch` は全件を 1 回で渡す。
`Executable::Batch` の `{files}` は path ごとに独立した引数へ展開し、`Association::Batch` は全 path を
1 つの `IDataObject` に載せる。`OsDefault::Batch` は `Each` と同じで、21 件以上の個別起動は
確認ダイアログを挟む。複数試行の結果は 1 利用者操作として成功 / 失敗件数を集約して通知する。

P2b-1 の `ExternalToolPicker` / `ExternalTool1` .. `ExternalTool10` /
`ExternalToolForContainer` は main-window の Grid 専用で、既定キーは設定しない。前二者はページ対象、
最後は現在の `effective_folder()` からフォルダー / 本 1 件を解決する。変換アーカイブでは変換 cache ZIP
でなく元アーカイブを渡し、検索・タグ・履歴・snapshot 等の集約ビュー背景では単一の現在地を決めず
拒否する。右クリックは従来どおり `show_in_context_menu` のツールを平坦に並べ、フォルダー背景と
コンテナー項目だけ対象をコンテナー 1 件へ切り替える。これらの入口を追加しても、ページ対象に含まれる
仮想ページの P3 までの全体拒否は変わらない。ツールバー / メニューバー / `OsDefault + Batch` の
設定 UI 説明は P2b-2 であり未実装。

## 5. スマートフォルダと検索

この節の「対象」は、現行 producer がその種別を候補として評価し、その種別の `GridItem` として
結果を materialize することをいう。親 folder を drill 表示用に合成するだけの場合や、親
PDF が別の `PdfFile` として検索できるだけの場合は、子 page 自体の対応とは数えない。

| 機能 | 実ファイル (`Image` / `Video` / `Audio`) | `Folder` | コンテナファイル (`ZipFile` / `PdfFile`) | `ZipImage` | `PdfPage` | `Stack` | `ZipDir` | 保存先・対象ファイルの変更 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| スマートフォルダの結果対象 (`src/app/smart_folder.rs:17-39`) | **対応**。3 種の実ファイルを分類・収集し、同じ variant へ materialize する (`src/app/smart_folder.rs:750-770`, `src/app/smart_folder.rs:941-952`, `src/app/smart_folder.rs:2112-2135`)。 | **対応**。実 directory を収集して `Folder` にする (`src/app/smart_folder.rs:883-940`, `src/app/smart_folder.rs:2112-2135`)。 | **対応**。実 ZIP/PDF を分類し `ZipFile` / `PdfFile` にする (`src/app/smart_folder.rs:750-770`, `src/app/smart_folder.rs:2112-2135`)。 | **拒否**。entry kind / materialize 先に variant がなく、仮想表示を source にする UI も理由付き disabled (`src/app/smart_folder.rs:17-39`, `src/app/smart_folder.rs:2112-2135`, `src/ui_dialogs/smart_folder_editor.rs:309-336`, `src/ui_main.rs:5363-5388`)。 | **拒否**。`ZipImage` と同じく entry kind / materialize 先がない (`src/app/smart_folder.rs:17-39`, `src/app/smart_folder.rs:2112-2135`)。 | **拒否**。`Stack` を結果にする entry kind / materialize 先がない (`src/app/smart_folder.rs:17-39`, `src/app/smart_folder.rs:2112-2135`)。 | **拒否**。`ZipDir` を結果にする entry kind / materialize 先がない (`src/app/smart_folder.rs:17-39`, `src/app/smart_folder.rs:2112-2135`)。 | 定義は `settings.db` の settings data に保存 (`src/settings.rs:268-287`, `src/settings.rs:3615-3618`, `src/settings_db.rs:779-837`)。結果は in-memory `SmartFolderSnapshot` (`src/app/smart_folder.rs:113-120`)。メディア非変更。 |
| Ctrl+S コンテナ名前検索 | **拒否**。Image/Audio は classifier に存在せず、Video は明示除外 (`src/name_bulk_indexer.rs:339-362`)。 | **対応**。directory を `Folder` として索引する (`src/name_bulk_indexer.rs:339-362`)。 | **対応**。`.zip` / `.cbz` / `.pdf` を索引する。以前は classifier が拡張子 `zip` の厳密一致で `.cbz` を落としていた (2026-08-30 修正、`src/name_bulk_indexer.rs` が `folder_tree::is_zip_extension` を共有)。 | **拒否**。内部 entry を列挙せず index kind / result variant にない (`src/search_index_db.rs:24-40`, `src/name_bulk_indexer.rs:261-362`)。 | **拒否**。ページを列挙せず `PdfFile` 単位だけを索引する (`src/name_bulk_indexer.rs:339-362`)。 | **拒否**。index kind / result materialize 経路にない (`src/search_index_db.rs:24-40`, `src/app.rs:20574-20645`)。 | **拒否**。index kind / result materialize 経路にない (`src/search_index_db.rs:24-40`, `src/app.rs:20574-20645`)。 | `<data_dir>/search_index.db` に path / name / kind を保存 (`src/search_index_db.rs:92-105`, `src/search_index_db.rs:147-213`)。結果は memory、メディア非変更。 |
| Ctrl+G アイテム / metadata 検索 (`src/search_walker.rs:289-323`) | **対応**。walker / ingest / result は Image/Video/Audio を持つ (`src/search_walker.rs:55-65`, `src/ingest_worker.rs:263-268`, `src/global_search_ui.rs:780-835`)。 | **拒否**。directory は traversal 用だけで hit 対象でない。descendant hit の親を drill / `SearchContainer` として合成する経路は Folder hit ではない (`src/search_walker.rs:289-323`, `src/global_search_ui.rs:848-927`)。 | `PdfFile` は **対応**、`ZipFile` は **拒否**。walker は PDF を候補にする一方 ZIP を明示 skip し、flat result も stale ZIP hit を捨てる (`src/search_walker.rs:289-323`, `src/global_search_ui.rs:780-835`)。 | **拒否**。walker は ZIP を skip して entry producer を持たず、stale hit 復元の defensive branch しかない (`src/search_walker.rs:304-323`, `src/ingest_worker.rs:371-387`, `src/global_search_ui.rs:930-960`)。 | **拒否**。PDF は file 単位で ingest / `PdfFile` 復元し、`PdfPage` producer はない (`src/ingest_worker.rs:326-387`, `src/global_search_ui.rs:780-835`)。 | **拒否**。flat / aggregate / drilled materialize のいずれも `Stack` を作らない (`src/global_search_ui.rs:752-835`, `src/global_search_ui.rs:848-927`)。 | **拒否**。flat / aggregate / drilled materialize のいずれも `ZipDir` を作らない (`src/global_search_ui.rs:752-835`, `src/global_search_ui.rs:848-927`)。 | `<data_dir>/fts_index/` と `fts_meta.db` (`src/indexer_manager.rs:192-244`, `src/fts_index.rs:330-341`, `src/fts_meta.rs:93-106`)。メディア非変更。 |
| Ctrl+F 現在グリッド filter (`src/app.rs:67614-67640`) | **対応**。Image/Video は filename + metadata、Audio は filename を on-demand 判定 (`src/app/metadata_ops.rs:1487-1497`, `src/app/metadata_ops.rs:1528-1637`)。 | **対応**。表示名で判定 (`src/app/metadata_ops.rs:1437-1450`)。 | **対応**。ZIP は filename、PDF は filename + document info (`src/app/metadata_ops.rs:1437-1485`, `src/app.rs:47366-47373`)。 | **対応**。ZIP entry basename で判定 (`src/app/metadata_ops.rs:1498-1507`)。 | PDF 自身を開いている間は **拒否** (Ctrl+F 自体を無効化)。★ 横断一覧など合成パスで開いた混在 view では **対応** (`Page N` matcher)。以前は判定が先頭 item だけだったため、混在 view で先頭が `PdfPage` かどうかという並び順の偶然で view 全体が無反応になっていた (2026-08-30 修正、`grid_is_pdf_pages` が `current_folder` を見る)。 | **対応**。stack key の表示名で判定 (`src/app/metadata_ops.rs:1437-1450`)。 | **対応**。directory last segment の表示名で判定 (`src/app/metadata_ops.rs:1437-1450`)。 | 現在の `App.items` snapshot を都度 worker で判定し、結果は memory の `search_filter: HashSet<usize>` (`src/app.rs:47321-47408`, `src/app.rs:47444-47474`)。永続 index / メディア変更なし。 |

Ctrl+S / Ctrl+G / Ctrl+F の新規検索入口は main-window handler にだけあり
(`src/app.rs:67614-67640`, `src/app.rs:67694-67724`)、fullscreen では同じ key chord が capture や
adjustment 等の別 action になる (`src/keymap.rs:5438-5446`, `src/ui_fullscreen.rs:20022-20025`,
`src/ui_fullscreen.rs:20835-20850`)。ただし検索結果や smart-folder result を開いた後の viewer は
同じ item 集合を使うため、上表の target membership 自体は変わらない
(`src/global_search_ui.rs:2023-2074`, `src/ui_fullscreen.rs:24812-24825`)。

## 6. 食い違いの一覧

> **2026-08-30 時点の進捗**: 5 / 8 / 11 / 12 は修正済み (本文の該当セルも更新済み)。
> 残りは主に **1 / 2 / 3 / 4 / 9 / 10 = タグとページの関係**に集約される。
> 14 (checked を無視する ExternalTool) と 15 (Windows 側削除) は現行仕様として意図的。

1. **★ / タグ × `ZipImage` / `PdfPage`** — ★ はページ固有 key・kind で付き、タグは
   viewer では親 ZIP/PDF に付き、グリッド quick action では何も起きない。利用者が同じページを
   指しても作用点が三通りになる (`src/app.rs:47605-47616`, `src/rating_db.rs:28-57`,
   `src/tag_ops.rs:21-40`, `src/tag_ops.rs:208-225`)。
2. **viewer metadata panel × ZIP/PDF page** — ★ は「親コンテナ」と「ページ」の二行を出すが、
   タグは「この本 / この PDF」の親一行だけを出す (`src/ui_metadata_panel.rs:275-350`,
   `src/ui_metadata_panel.rs:701-716`, `src/ui_metadata_panel.rs:787-805`)。
3. **タグ × checkable page** — `ZipImage` / `PdfPage` は checkbox を持つのに、グリッド tag
   resolver から黙って脱落する。混在一括では実ファイルだけが変更される
   (`src/grid_item.rs:310-318`, `src/tag_ops.rs:133-148`)。
4. **タグ × page / `Stack` / `ZipDir` の入口差** — 通常 menu は disabled、`T` はトースト、
   ピン留め quick action は無反応になる (`src/ui_main.rs:5629-5643`,
   `src/ui_dialogs/tag_apply.rs:23-31`, `src/app/gamepad_input.rs:5168-5186`,
   `src/tag_ops.rs:208-225`)。
5. **削除 × `ZipImage` / `PdfPage`** — 単一右クリックでは項目非表示、単一 `Delete` key は
   無反応、checked では削除項目が出た後にトーストで全体拒否となる
   (`src/context_menu_model.rs:263-311`, `src/context_menu_model.rs:476-487`,
   `src/ui_dialogs/context_menu.rs:1532-1544`, `src/ui_dialogs/context_menu.rs:1897-1916`)。
6. **実ファイル操作 × checkable page** — `file_operation_path` / `drag_source_path` は page を
   除外する一方、`is_checkable` は page を含む。そのため checked 後に Copy/Cut、パスコピー、
   削除が実行時拒否になる (`src/grid_item.rs:270-318`, `src/app.rs:36426-36461`,
   `src/ui_dialogs/context_menu.rs:1366-1377`, `src/ui_dialogs/context_menu.rs:1532-1544`)。
7. **仮想 item 混在選択 × file 操作** — Copy/Cut、パスコピー、削除は全体拒否するが、D&D は
   仮想 page だけ除外して実 path を部分実行し、後からトーストを出す
   (`src/app.rs:36426-36461`, `src/ui_dialogs/context_menu.rs:1366-1377`,
   `src/ui_dialogs/context_menu.rs:1532-1544`, `src/ui_main.rs:1705-1735`)。
8. **単一 D&D / `Delete` × `Stack` / `ZipDir`** — resolver が path を返さないまま通知なしで
   終わる。一方 Ctrl+C / Ctrl+X は同じ対象をトーストで拒否する
   (`src/grid_item.rs:288-307`, `src/ui_main.rs:1736-1743`,
   `src/ui_dialogs/context_menu.rs:1907-1916`, `src/app.rs:36438-36461`)。
9. **★ / タグ × `Stack` / `ZipDir`** — `Stack` は ★ action が発火できても対象 0 件で無反応、
   `ZipDir` は合成 key で ★ 対応する。一方タグは両方 target を持たず、子 ZIP page の viewer
   タグも `ZipDir` prefix でなく外側 ZIP へ付く (`src/grid_item.rs:167-214`,
   `src/app.rs:47623-47630`, `src/app.rs:49565-49573`, `src/tag_ops.rs:21-40`)。
10. **横断一覧 × page** — rating 一覧は `ZipImage` / `PdfPage` / `ZipDir` を variant のまま
    復元して実 item と同じ一覧に並べるが、tag 一覧は実 path 系 kind しか復元しない
    (`src/rating_view.rs:174-225`, `src/tag_view.rs:228-260`)。
11. **Ctrl+F × archive page / 混在一覧** — `ZipImage` / `ZipDir` は名前 filter に対応する一方、
    通常の PDF page grid は Ctrl+F が無反応である。さらに PDF 判定は先頭 item だけなので、
    rating の混在一覧では先頭が非 `PdfPage` なら `Page N` matcher へ到達し、先頭が
    `PdfPage` なら非 page を含む一覧全体が無反応になる
    (`src/app.rs:47417-47424`, `src/app.rs:67614-67640`,
    `src/app/metadata_ops.rs:1437-1450`, `src/app/metadata_ops.rs:1498-1515`,
    `src/rating_view.rs:174-225`)。
12. **Ctrl+S × `ZipFile`** —通常の ZIP 判定は `.zip` と `.cbz` を Zip とするが、Ctrl+S の
    classifier は拡張子が厳密に `zip` の場合しか索引しない。そのため同じ `ZipFile` variant
    でも `.cbz` は検索されない (`src/folder_tree.rs:137-141`,
    `src/name_bulk_indexer.rs:339-362`)。
13. **ExternalTool × virtual page / `Stack` / `ZipDir`** — P3 の実体化までは仮想対象として扱う。
    単独では閲覧 tool が menu から消え、編集 tool だけ理由付き disabled になる。実項目との混在では
    tool の入口を残すが、起動境界で選択全体を理由付き拒否する。親 fallback や部分実行はない
    (`LaunchTarget::Virtual`, `virtual_target_error`)。
14. **ExternalTool × checked selection × menu 対象全種別 (P2a で解消)** — menu 構築時に checked
    全件を `current_grid_order()` 順で snapshot し、右クリック項目が checked 内なら先頭へ移す。
    menu 表示後に checked が変わっても dispatch は同じ snapshot を使い、`SelectionPolicy` を適用する
    (`resolve_external_targets`, `NativeGridContextMenuTarget::external_tool_targets`)。
15. **mIV 削除 / Windows Shell 削除 × 実 path** — 同じ native context menu に両方が併存し得るが、
    Windows 側 command は mIV の metadata hard-purge 経路を通らない。Windows が削除項目を
    出すか自体は環境依存で **要調査** (`src/native_context_menu.rs:415-451`,
    `src/native_context_menu.rs:561-605`, `src/delete_worker.rs:250-309`)。
