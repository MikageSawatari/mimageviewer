# §1.256 一覧の本サムネイルの読書位置メーター — 記録値を表示する設計

作成・改訂: 2026-10-04。コード調査基準: `next-file-ops` / `e804db069`。
状態: **実装完了・関連自動検証済み。全体ゲートはlauncher入力用release binaryの準備待ち。** 独立設計レビュー (`gpt-6.1-sol` / `xhigh`) のP2 2件と2026-10-04の設計担当決定を反映済み。後続独立レビューの内容identity復元P2も、既存延期機構がないため利用者指定の割り切りで対応 (§4.2 / §7 / §9)。通常削除の競合も利用者合意済み。確認用ビルドは前回完了。commit・アプリ起動は行っていない。
要件: [next-release-backlog.md §1.256](next-release-backlog.md#1256-一覧の本サムネイルに前回読んだ位置のメーターを表示する--438-2026-09-19)。本書の仕様判断は、2026-10-04の利用者合意によって以前の厳密な内容照合案を置き換える。file:line は調査基準時点のコード事実、追加する型・列・APIは提案である。

## 0. 合意した設計と前提

**実装前提の矛盾 (2026-10-04確認): 通常の削除には「移行前にwriterの処理済みを待つ」経路がない。** `rename_migration_writers_busy` (`src/app.rs:40444`) はrename開始 (`:40507`)、purge retry開始 (`:41034`)、明示メタデータ整理 (`src/ui_dialogs/metadata_cleanup.rs:108`)、明示メタデータ転記 (`src/ui_dialogs/metadata_transfer.rs:303`) に使われる。このpredicateにBookResumeWriterを加える決定は実施可能。一方、通常削除の `start_delete_files` (`src/app.rs:41057`) はjournalのadmissionだけを確認 (`:41069`) し、writer待機なしにdelete workerをspawnする。workerはShell削除後、末尾で直接purge (`src/delete_worker.rs:298`) する。`acquire_delete_epub_guard` (`:203`) はEPUBの範囲保護で、BookResumeWriterのRecord完了を待つものではない。このためpredicateへの追加だけでは「未処理Record → 通常delete/purge → 旧keyへのRecord commit」の復活を防げない。

2026-10-04の追加決定: 通常削除への共通待機拡張は採用しない。短い競合で存在しないpathの行が残ることを許容し、削除時にはmapの該当scopeを消す。rename / purge retryなど既存の待機経路だけにBookResumeWriterを加える。新しい未開始削除要求・専用barrier・UI待機を作らない (§7)。

**記録時の「何ページ目 / 全何ページ / 右綴じ」を保存し、一覧はその値を表示する。** 読み順や中身を後から数え直さない。起動時の全行読込と稀なDB変更後の再読込を既存writerで行い、通常のスクロール・描画はメモリmap参照だけにする。

利用者了承済みの割り切り: 旧行の追加列がNULLなら次に読むまで不表示。読んだ後に内容や並びが変わっても次の記録までは保存した比率を表示。旧版へ戻して読んだときに新列の古い値が残っても許容する。移行失敗はログとメーター非表示で扱い、従来の位置復元は維持する。proof JSON、trigger、内容版・manifest、監視、未開封本の方向解決、可視範囲worker、これらに付随する世代管理は採用しない。

前提のコード上の注意: 保存済み `page` はページ番号ではなくraw `items` index。HUDには要求どおりの読めるページ列を使う計算があり (§1.5)、これを再利用できる。一方Remoteの検証結果にはordinal/totalはあるが右綴じ情報が無い (§6)。初版は合意されたNULL保存を選ぶ。backlog §2.2 の「右上/右下未着手」は現行layoutと異なるため、現行 `ThumbnailOverlayLayout` を基準に下端の帯だけを足す (§5)。記録値の表示方式は実施可能で、通常削除の順序保証は上記の割り切りで判断済み。

## 1. 現状のコード事実

### 1.1 保存するものと書き込み入口

| 事実 | 根拠 |
| --- | --- |
| 保存先は `data_dir/book_resume.db`。テーブルは `book_resume(path TEXT PRIMARY KEY, page INTEGER NOT NULL DEFAULT 0)` の2列 | `src/book_resume_db.rs:38`, `src/book_resume_db.rs:47`, `src/book_resume_db.rs:56` |
| キーは `path_key::normalize`。ドライブ文字除去、小文字化、区切り統一。別ドライブの同名パスが同一キーになる既存の仕様 | `src/book_resume_db.rs:7`, `src/book_resume_db.rs:106`, `src/path_key.rs:20` |
| `get` は None とエラーを区別せず、負の page は0へ丸める。メーター照会ではこの丸め・エラー隠蔽を継承しない | `src/book_resume_db.rs:61` |
| App は起動時に読取接続と専用 writer を作る。writer は FIFO channel で `(PathBuf, usize)` を受けて upsert、終了時 drain / join | `src/app.rs:17066`, `src/book_resume_db.rs:116`, `src/book_resume_db.rs:129`, `src/book_resume_db.rs:167` |
| ローカルの共通入口は `record_book_resume(idx)`。Image / ZipImage / PdfPage だけ、ZIP の非 root 階層では記録せず、`current_folder` と idx で dedup | `src/app.rs:50820`, `src/app.rs:50831` |
| fullscreen open、ページ移動確定、連結読みの位置変更で記録する。close 時だけの記録でも、過去最大ページの記録でもない | `src/app.rs:59265`, `src/ui_fullscreen.rs:20985`, `src/ui_fullscreen.rs:38058` |
| Remote に別の producer があり、検証後の container path / raw page index を同じ writer へ記録する | `src/remote_ipc/ui.rs:2733`, `src/remote_ipc/ui.rs:2754` |
| 削除・rename・copy では共通 store registry がこの DB の path 列を DriveStripped として扱う | `src/rename_key_migration.rs:1067` |

`record_book_resume` は通常の混在フォルダやスタック由来の画像を一般的に拒否する関数ではない。履歴 OFF・復元を「先頭から」にしても、保存可能な画像の位置記録とは別条件である。`last_book_resume` は最後の1件の dedup / Remote 直近読取用で、全本の位置 cache に転用できない (`src/app.rs:14778`, `src/remote_ipc/ui.rs:1891`)。

### 1.2 index、ソート、絞り込み、見開きの関係

- 通常フォルダの scan はフォルダと各媒体を分類し、sort・同名媒体の優先・画像拡張子重複除去を経て `items` にする。製本フォルダではコンテナを除外して画像だけ、Numeric 順。画像のみフォルダを本扱いする設定が実効 ON のときは FileName の自然順でページ順を固定する。通常 browse は選択ソートの影響を受ける (`src/app/folder_scan.rs:233`, `src/app/folder_scan.rs:254`, `src/app/folder_scan.rs:289`, `src/app.rs:23`, `src/app.rs:43`)。
- 絞り込みは基本的に `items` の物理 index を残し、`visible_indices` を作る。詳細表示では `details_order` が表示・読み順となり得る。`current_reader_order` は collection root の特例以外 `current_grid_order` を使う (`src/app.rs:61440`, `src/app.rs:62530`, `src/app.rs:62544`)。よって「index=読み順の何番目」ではない。
- 本の物理ページ順が固定される境界は `physical_page_order_locked` / `page_order_locked_for_current_view`。通常の一覧ソートと、本の内部ページ順を分離する既存境界を使う (`src/app.rs:11111`, `src/app.rs:41768`)。
- ZIP loader は画像を再帰列挙するが、その件数は全階層の画像数。実際の viewer は `ZipTree` の root-effective prefix を collapse し、現在階層の子 `ZipDir` → 直下 `ZipImage` を FileName 順で materialize する。`at_root` は prefix が空かではなく stack 深さ1かで決まる (`src/zip_loader.rs:1010`, `src/zip_tree.rs:146`, `src/zip_tree.rs:180`, `src/zip_tree.rs:294`, `src/zip_tree.rs:307`, `src/app.rs:31414`)。
- スタックは集約セルを開くと全画像の flat `items` に差し替え、閉じると集約へ戻す。raw index の意味を通常 scan の index と同一視できない (`src/filename_stack_ui.rs:4`, `src/filename_stack.rs:234`, `docs/filename-stack-plan.md:18`)。
- 見開き・連結読みでも保存するのは実ページの navigation anchor の idx。相手ページや末尾に添える表紙を `record_book_resume` に足していない (`src/ui_fullscreen.rs:38054`, `src/app.rs:59266`, `CHANGELOG.md:47`, `CHANGELOG.md:105`)。

### 1.3 復元と履歴の差

`resume_page_for_container` は `current_folder` の保存 idx を取得して `is_readable_page_idx` で検証する。範囲外・非画像は None。deferred open はこの idx を優先し、なければ通常の初期ページ探索へ進む。グリッドの初回選択復元も同じ種別確認を使い、その後に可視選択へ redirect する (`src/app.rs:51028`, `src/app.rs:32624`, `src/app.rs:35071`)。メーターは先頭への fallback を既読位置として描かない。

`reading_history.db` は KeepDrive の key と path / kind / 最終日時 / 1-based last_page / page_count / 媒体進捗 / metadata を持つ。位置は `current_reader_order` を走査して数える。履歴 OFF、上限1000、30秒の touch 制限があり、非 root ZIP ではページ位置を保存しない。通常一覧の正本にはしない (`src/reading_history_db.rs:17`, `src/reading_history_db.rs:23`, `src/reading_history_db.rs:60`, `src/reading_history_db.rs:143`, `src/app.rs:50861`, `src/app.rs:50958`)。

### 1.4 ページ総数と既存非同期取得

| 対象 | 現状の取得源・注意 |
| --- | --- |
| PDF | 親 catalog の `pdf_meta(filename, mtime, file_size, page_count, password_required)`。stamp 一致で返す。cache miss は PDF enumerate worker。保存済みファイル固有 password と credential revision を使用、session-only password は使わない (`src/catalog.rs:994`, `src/app/metadata_ops.rs:1466`, `docs/pdf-page-count-cache-plan.md:77`) |
| ZIP | `container_page_meta` の kind / mtime / size / 画像認識 fingerprint。一致なら count、なければ `enumerate_image_entries().len()`。**この全階層 count だけでは root のページ数・index 対応を証明できない** (`src/catalog.rs:1120`, `src/app/metadata_ops.rs:1323`) |
| 画像フォルダ | 共通 scan で「子コンテナなし、認識媒体が空でなく全て画像」を確認し、同じ拡張子重複規則で count。無関係な非対応ファイルは無視。親 catalog に count / 非本の None を保存 (`src/app/folder_scan.rs:449`, `src/app/folder_scan.rs:500`, `src/app/folder_scan.rs:530`, `src/app/metadata_ops.rs:1368`) |
| 直接閲覧 RAR | inspect が Direct と判定した場合の画像 count。変換が必要な RAR、7z/LZH 全体をこの列が数えるわけではない (`src/app/metadata_ops.rs:1423`, `src/app.rs:61180`) |
| ZIP 内子書庫 | warm な `ZipTree::page_count_for_prefix_str`。これも部分木総数であり、resume root index の対応証明とは別 (`src/app.rs:61263`, `src/zip_tree.rs:114`) |

詳細表示の遅延 worker は field ごとの target、generation と正規化 key の結果、8 parent catalog LRU、I/O semaphore / cancel を持つ。page-count-only では可視 stage、ページ数ソートでは全件取得になる。サムネイル tooltip / 選択情報 / 長さ表示などの要求も同じ基盤に入る (`src/app.rs:6268`, `src/app.rs:6293`, `src/app.rs:60662`, `src/app/metadata_ops.rs:526`, `src/app/metadata_ops.rs:1540`)。フォルダ count cache の stamp はフォルダ自身の metadata であり、子画像の全変更を表す保証ではない。

### 1.5 HUDの計算と再利用する境界 (追加調査)

- `GridItem::has_page_data` はImage / ZipImage / PdfPageだけ (`src/grid_item.rs:220`)。
- `get_still_image_indices()` は `current_reader_order()` を `ui_helpers::still_image_display_indices` に渡す。helperは入力順を維持して上記3種だけを抜き出す。filter・詳細ソート・stack flat等の実際の読み順を使い、raw index順へ組み直さない (`src/ui_fullscreen.rs:27569`, `src/ui_helpers.rs:1531`, `src/app.rs:62544`)。
- `fullscreen_seek_info` はこの列の `.position(idx)` を0-based current_pos、`.len()` を画像総数としてHUDに渡す。動画/その他の件数は別に数える (`src/ui_fullscreen.rs:20742`)。
- `image_reading_position(image_indices, idx)` は同列内のposition+1。`fullscreen_page_number_label_for_info` は `.len()` を分母とし、単ページ/連結ならanchorの位置、見開きなら両実ページの位置を表示する (`src/ui_fullscreen.rs:16055`, `src/ui_fullscreen.rs:20790`)。
- 画像列とseek infoは既存 `ViewerNavigationCaches` に保持される (`src/ui_fullscreen.rs:15944`, `src/ui_fullscreen.rs:20763`)。本件専用の別の読み順cacheや新しい索引は作らない。

提案する記録用helperは「既存 `get_still_image_indices` + `image_reading_position` + `.len()`」を返す薄い共通入口とする。HUDのラベル文字列をparseせず、計算を共有する。見開きラベルに2ページ載っても記録するのはanchorのordinalだけ。`current_reading_history_page_position` は途中に非画像があるとNoneにするため、混在フォルダも扱う今回の記録計算にはそのまま使わない (`src/app.rs:50958`)。

### 1.6 mapに影響する既存書き込み経路 (追加調査)

- ローカルは `record_book_resume`、Remoteは `persist_remote_reading_progress` の別入口 (`src/app.rs:50831`, `src/remote_ipc/ui.rs:2733`)。
- 全件クリアは現状Preferencesから `book_resume_db.clear_all()` を直接呼ぶ (`src/ui_dialogs/preferences.rs:2811`)。
- delete workerのDB purge完了後にAppがメモリcacheを整理し、rename移行完了でもpresence/resume keyを更新する (`src/app.rs:41202`, `src/app.rs:40647`, `src/app.rs:40702`)。
- 共通store registryはbook_resumeのpathをDriveStrippedで扱う。copyはPRAGMA由来の全列をコピーするので追加列も運ばれる (`src/rename_key_migration.rs:1067`, `src/rename_key_migration.rs:2226`, `src/rename_key_migration.rs:2231`)。内容identityからの編集復元も `copy_stores_at_with_progress` を使う (`src/content_identity/restore.rs:181`)。
- D&D/貼付copyの完了は `poll_drop_copy_pending` で受ける。通常のファイルcopyとstoreの転記を区別し、DBにresume行をコピーした完了経路には再読込を接続する (`src/app.rs:41406`)。

## 2. 記録値・対象セル・鮮度の契約

保存する補助値は `ReadingMeterValue { ordinal, total, rtl }`。`ordinal` はHUDと同じ読めるページ列での1-based位置、`total` はその列の長さ、`rtl` は記録時の実効 `reading_direction == ReadingDirection::Rtl`。`spread_mode.is_rtl()` だけではSingleの右綴じを落とすため使わない。記録時に既にあるdirectionを使い、追加DB readをしない。

raw `page` と復元処理は変更しない。補助値は実際の読書時点のsnapshotであり、「現在の内容ならraw indexがどのページへ復元されるか」を保証する値ではない。読み順のfilter・sortに従って分母も変わる。親の一覧を絞り込み/並べ替えただけでは保存値を書き換えない。戻って読み直すとordinalも比率も減る。最大到達ページ・読了判定ではない。

見開きは記録されるanchorで数える。2–3ページでanchor=2なら2/N。最終見開きでもanchor=N−1なら(N−1)/N、anchor=Nなら100%。相手ページを推測して+1しない。添えた表紙・白slot・Splitの左右半面を増分として数えない。1ページ本の有効記録は1/1。

| 一覧セル/閲覧種別 | 初版の扱い |
| --- | --- |
| 通常一覧のFolder | 対象。本扱いON/OFFを問わない。画像だけでも、動画・子フォルダ・非画像が混ざっていても、記録時HUDの読めるページだけで数える |
| 製本フォルダのFolder | 同じmap参照で対象。製本の実際の読み順で記録する。追加・並べ替え後も再記録までは保存値を表示 |
| ZipFile / PdfFile | 当該実パスのresume keyでmapを引いて表示。ページ数列/catalog/passwordの再取得をしない |
| ZIPのroot / 単一wrapper root | 現状記録する範囲を維持。rootにZipDirが混じっていても、記録時に送り得るZipImageだけで数える |
| 入れ子ZIP内側 | 現状 `record_book_resume` が記録しないため対象外。その閲覧で外側rootの過去記録を消す処理も足さない |
| Stackセル / Image / ZipImage / PdfPage / ZipDir個別セル | メーターを描かない。flat stack閲覧が従来記録する値はHUDの読み順で補助値も記録でき、後の通常Folderセルに表示される。stack専用keyを新設しない |
| ConvertibleArchive (直接閲覧RARを含む) | 初版の対象セルに含めない。変換cache ZIPと元書庫のkeyを解く処理も作らない。既存の位置記録・復元は維持 |
| PdfFile扱いのEPUB | resume保存keyと当該cell pathが一致して行があれば同じmap参照で表示。変換generation/内容を解き直さず、異なるkeyを推測で結ばない |
| 詳細行・seek strip・Remote Web一覧・合成ビュー専用表示 | 今回の描画変更の対象外。通常物理一覧のFolder/ZipFile/PdfFileセルに限定。Tag/Smart/Collection等から入った物理子フォルダも入口を問わず対象、合成rootは非対象 (既存surface/positionとinstalled itemflagsを参照) |

行無し、追加列が1つでもNULL、total==0、不正値 (ordinal<=0 / ordinal>total / rtlが0・1以外) ではtrackも含め描かない。0%への代用やclampはしない。既に保存された有効値は、内容の変更・外部削除・password状態・認識規則変更等と再照合しない。通常の一覧更新によりcellが消えると描画も消えるだけで、本ごとの監視は不要。

## 3. 永続化・記録入口

### 3.1 追加列とwriter起動時の移行

リリース済み確認: `CHANGELOG.md:599` のv1.1.0内 `:604` に読書位置復元、`git show v2.6.0:src/book_resume_db.rs` に既存2列表を確認。旧path/pageを保持し、writer起動時に次のnullable列を足す。

```sql
ALTER TABLE book_resume ADD COLUMN page_ordinal INTEGER;
ALTER TABLE book_resume ADD COLUMN page_total INTEGER;
ALTER TABLE book_resume ADD COLUMN reading_rtl INTEGER;
```

既存writer自身の接続でPRAGMA table_infoを見て不足列だけ追加する。3列追加は1transactionで行い、旧行はNULL、新DBも同じ経路を使う。UI側の既存open/getにALTERや追加列照会を足さない。移行成功後に初回全行読込を行う。移行失敗ならログ、メーター利用不能とし、従来2列へのraw記録/SELECTと復元を継続する。読書位置を削除・初期化しない。trigger、JSON、schema専用journal、再試行loopを作らない。旧版のpage-only upsertで新列の古い値が残るのは了承済み。

### 3.2 ローカルの記録

`record_book_resume` の既存画像種別/ZIP root条件を維持し、§1.5の共通計算からordinal/total、現在のdirectionからrtlを取得する。計算できなければ補助値無しとする。追加scan、read_dir、PDF/ZIP列挙、page-count DB readを行わない。HUDと同じcached列を参照し、必要なposition検索は記録時だけ行う。ページ送り毎の追加全列clone/別のfilter処理を作らない。

writerへ `path / raw_idx / Option<ReadingMeterValue>` を送り、UI側のmapも同じ共通入口で更新する。既存 `last_book_resume` のdedupをpath/raw_idxだけで済ませず、補助値を含むrecord全体で比較する。同じidxでも読み順の分母・ordinal・direction、NULL→有効値が変われば更新する。direction変更は次の位置記録で保存する (変更だけを理由に全本の値を書き直さない)。ordinal/totalはusizeからSQLite INTEGERへのchecked変換を行い、表現できない値を丸めず補助値無しで記録する。

通常モードのwriterは1回のupsertでraw pageと追加3列を同時保存する。補助値無しなら3列ともNULLに上書きし、旧meterを残さない。未移行モードのwriterは従来pageだけを書き、メーターは描画しない。書込を受け付けた最新値をmapへ即時反映するため、一覧へ戻るとDB commitを待たずに直近の位置が出る。書込失敗の割り切りは§7。

## 4. 一覧用メモリmapと稀な更新

### 4.1 所有と初回読込

App全体が正規化keyから保存補助値Optionへのmapを1つ持つ。mapと読込受付の所有者はBoxに置き、既存Appのstackサイズ上限を維持する (状態・所有・処理経路は不変)。NULL/不正値行はNoneとして保持し、表示しないが登録件数をmap.lenで求める (UIでCOUNTしない)。初回読込前/初期化失敗はmapのOptionがNone、利用可能時はSomeとし、別の準備完了/失敗boolを足さない。main/detachedのitemsやnavigation cacheにmapを複製しない。各viewerで記録する共通入口が同じmapを更新する。キーは既存book_resumeと同じDriveStripped正規化で、別ドライブ同名pathの衝突も現行仕様を引き継ぐ。

既存 `BookResumeWriter` に全行読込command/結果を加え、新規workerを作らない。起動時は移行後にSELECTを1回行い、追加3列が有効な行のmapを構築してUIへ返す。初回のmapがまだ無くても一覧は通常表示し、到着後にrepaintする。初回待機中の記録は§4.3の差分へ保持し、移行/初回SELECT成功後のsnapshotに反映して初めて描く。初期化失敗ではNoneのままで、raw記録は続けてもmeterだけは出さない。UI側はDBを読まない。表示OFFでも起動時の1回読込と記録時更新を行い、ONへ戻す際の追加loadを不要にする。

### 4.2 書き込み経路別の同期方法

| 経路 | mapの最小更新 |
| --- | --- |
| ローカル記録 | 同じ受付入口でkeyをinsert、補助値無しならNoneとして保持し非表示。writerへ同じpayload |
| Remote記録 | §6のNULL記録を同じ入口へ渡し、そのkeyの値をNoneとして非表示。record_resume=falseなら変更無し |
| 読書位置の全件クリア | 既存writerへClear commandとして直列化し、map.clearとdedup解除を同じ受付で行う。結果で件数/メッセージを更新。UIでclear_all/countしない |
| deleteのstore purge完了 | 成功pathのexact / 配下 / `::` scopeに対応するmap keyをremove。既存metadata cache整理の完了点に接続。失敗/一部purgeの結果が不確かな場合は下記全行再読込 |
| rename/moveのstore移行・回復完了 | completion時に旧/新scopeの表示cacheを無効化し、writerで全行再読込。既存重複keyの扱いをUIで別実装しない |
| storeのcopy / 内容identityからの復元 | DB転記完了後だけ全行再読込。コピー先既存行優先などproduction SQLの結果を採用。内容identity復元は開始前のwriter待機・延期がなく、未処理Recordとの競合は§7の合意済み割り切り。通常file copyがresume DBを変更しないならこのための再読込は不要 |
| 設定リセット/復元・データストア再接続 | resume DBをクリア/置換/再接続する経路ならmapもclearして、新接続workerで全行再読込。メーターcheckboxのdefault/resetだけならmapを消さない |

rename/copyの全列移行は既存store registryを使用し、追加3列も一緒に運ぶ。稀な操作後の全行SELECTは許容し、独自prefix移行SQLや新しい差分DB監視は作らない。移行中は影響scopeの古いcell値を出さず、結果でrepaintする。他viewerのitems/worker/選択は変更しない。起動時の既存移行回復・後続cleanup完了も同じ再読込入口へ接続する。

**独立レビューP2の決定:** BookResumeWriterの未処理commandを既存 `rename_migration_writers_busy` に含める。enqueueからDB処理完了までを数え、worker内のcommit完了後に解除する。UIは既存のpredicate/poll経路で開始を繰り延べ、待機・join・DB照会をしない。既存待機があるrename / 明示メタデータ転記 / purge retry等では、Recordが旧keyへ着地してから操作が始まる順序を維持する。book_resume専用の直列化境界は作らない。通常deleteと内容identity復元は§7の割り切りを適用する。

**後続レビューP2の決定:** 内容identity復元には小さく流用できる延期機構がない。`src/app/content_identity_restore.rs:281` は確認ボタンでpromptをtakeし、選択結果を `:319` の開始入口へ渡して `:347` でworkerをspawnする。`ContentIdentityRestorePending` (`:65`) は起動済みworkerのreceiverだけを保持し、`:366` のpollはその結果を待つだけ。renameのqueue/pollや明示メタデータ転記の `Stage::WaitingForWriters` (`src/ui_dialogs/metadata_transfer.rs:337`) とは別ownerである。共通busy predicateだけを追加すると確認済み要求を失うため、接続には未開始のselected/declined要求を保持し再開する新状態が必要。利用者の条件付き決定に従い接続しない。通常版・EPUB版とも同じ開始境界を通り、転記は `src/content_identity/restore.rs:181` 等の別接続で行う。復元完了後のmap再読込は維持する。

後続cleanupの接続先は `src/ui_dialogs/metadata_cleanup.rs:59` の明示メタデータ整理と `src/app.rs:40982` のpurge retry完了も含める。どちらもDB変更の完了後に全行再読込し、部分成功でも削除済行をmapに残さない。

### 4.3 非同期の全行読込に必要な最小限の相関

初回/再読込の古いsnapshotが、その待機中に記録した最新値やClearを上書きしないことだけを扱う。1つのpending receiverに **読込要求後のmap更新 (Set/Remove/Clear/RemoveScope)** を保持し、到着したsnapshotへ受付順に適用してからmapを置き換える。同じpending中の更新は表示mapにも直ちに適用する。これらはmap操作だけで、content/読み順のproofではない。

全行読込commandは既存writerのFIFOで、それまでのRecord/Clear後に実行する。外部store移行の完了後に要求するためその確定DBを読む。待機中に別の稀な操作が完了したら最新の全行読込を要求し、古いreceiverを捨てる。新要求より前のRecord/Clearはwriter FIFO、新要求後の更新はpending差分で守られる。receiverを分けるのでrequest世代・folder/view/range世代を追加しない。アプリ終了はreceiverを捨て、writerの既存終了drainを維持する。UIでjoin/waitしない。

map更新はAppの単一受付へ集約し、起動読込中・再読込中のSet/Remove/Clear/RemoveScopeを全て通す。比較的稀な操作のscope整理がmap全件を走査しても、毎frameの描画には入れない。新しいrollback・resume・supersession状態機械やretry loopは不要。

### 4.4 描画ホットパス

可視Folder/ZipFile/PdfFileのkeyをmapで引くだけ。既存itemsのpathからの正規化は毎paintで文字列allocateせず、メーター用keyは実path自身をkeyにして初回にmemoする。idxやviewerをkeyにしないためitems差替え/削除/並べ替えで別pathへ流用せず、専用generationも不要。稀なscope削除時にmemoをclearする。全items分を初回にscan/cloneする必要はない。

保存keyの再計算はitems差替え/新cell描画時だけ。通常frameは可視セル数×key参照/map lookup/矩形描画程度。スクロールworker・本ごとの監視・catalog接続・GPU texture生成はゼロ。起動/稀なDB再読込の完了時と既存の記録/設定確定時にrepaintし、メーター取得のためのidle heartbeatを作らない。

## 5. 描画・方向・設定

### 5.1 下端の専用領域 (前案を維持)

`layout_cell_overlays` は `rect.shrink(4)` のinnerと実測badgeを純layoutへ渡し、`draw_cell` は同じlayoutをcell内へclipする (`src/app/grid_paint.rs:254`, `src/app/grid_paint.rs:383`)。現行 `ThumbnailOverlayLayout` はcheck/stack/top-left/bottom-left/filter count/media durationを所有する (`src/thumb_overlay_layout.rs:184`, `src/thumb_overlay_layout.rs:426`)。ここにmeter rectを追加し、下端帯の所有を1箇所にする。backlog §2.2の古い未着手記録を理由に四隅を再実装しない。

inner下端の高さ3 logical pt、左右はinner端、上に2pt gapを初期値とする。meterがある時だけ `badges_inner.max.y = meter.top - 2pt` として既存layoutへ予約を渡す。**独立レビューP2の決定:** 右下filter件数は `cell.max.y - 3` を基準にする (`src/thumb_overlay_layout.rs:712`, `:758`) ため、innerの縮小だけで済ませず、右下配置の基準にもmeter帯 + gapの予約を純layout内で反映する。形式/フォルダ名/filename/評価、右下filter countはその上に置く。左上編集/pin/tag/time/UP、右上check/stackは既存優先規則を維持。動画/音声cellは非対象で、長さ表示と共存しないが共有duration layoutの回帰も確認する。極小cellでは既存badgeを優先し、meterを省略する。

cell_h・cell rect・並び順・image fit・scroll content・hit-testは不変。回転/補正済bitmapやcatalog thumbnailに焼き込まない。選択borderは上層、cutは既存content painterのopacity、タグhit-testは同じBadgePlacementを使う。狭いcellでもmeterとbadgeは重ねない。

### 5.2 保存された右綴じを使う方向

`fullscreen_seek_direction.is_rtl(if saved.rtl { Rtl } else { Ltr })` を使う (`src/settings.rs:3480`, `src/settings.rs:3510`, `src/ui_fullscreen.rs:30568`)。FollowReadingは保存RTLなら右→左、それ以外は左→右。LeftToRightは常に左→右。現在開いている別本のdirectionや未開封本のspread DB/PDF文書方向を読み直さない。

比率はordinal/total。1/N、途中、N/Nをそのまま描き、向きはrectの塗り起点だけを変える。seek方向設定の変更は全可視cellの次paintへ即時反映し、map/DBは変更しない。本の綴じ方向自体は次の位置記録で保存値が更新される。

### 5.3 色・設定・再描画 (前案を維持)

色は `os_theme::book_resume_meter_palette(effective_dark)` 相当のsemantic helperで所有し、paint側へLight/DarkのRGB分岐を分散させない (`src/os_theme.rs:291`, `src/os_theme.rs:343`)。trackは不透明の暗灰/明灰、fillはテーマ別青緑、1px境界を候補にsnapshotで確定する。白/黒/鮮やかな表紙上でもfillと未塗りを区別する。テーマは当該UIのresolved visualsを使い、OSテーマ固定値をcacheしない。テーマ変更はpalette再取得だけ。

設定は **`thumb_show_book_resume_meter`、既定ON**、全体共通。環境設定 **表示 → サムネイル** に **「本のサムネイルに前回の読書位置を表示」**。説明は「記録されたページ位置を表示します。未読・位置やページ数を確認できない本には表示しません」。favorite/本別設定・方向独立設定は増やさない。閲覧表示の「ページシークバーの方向」説明にmeterにも適用する旨を足す (`src/ui_dialogs/preferences/pages.rs:1359`, `src/ui_dialogs/preferences/pages.rs:1623`, `src/ui_dialogs/preferences/pages.rs:9504`)。

既存draft編集→OK→prepare/merge→ `install_preferences_settings` → `settings.save()` を使う (`src/ui_dialogs/preferences.rs:1924`, `src/ui_dialogs/preferences.rs:1957`, `src/ui_dialogs/preferences.rs:2448`, `src/ui_dialogs/preferences.rs:2592`)。OFFはpaintを止めるだけでmap/記録は保持、ONは保持mapから表示する。Cancelはruntimeへ適用しない。serde欠落既定値・settings.db roundtrip・default/reset・Preferences管理フィールドとしてのmergeを揃え、既存設定確定のrepaint経路へ接続する。

## 6. Remoteの選択: 初版は追加3列をNULL保存

Remoteの `ValidatedPageContext` はraw page_index / 1-based page_number / page_count /記録可否を持つが、rtlを持たない (`src/remote_ipc/container.rs:1985`)。Folderはmaterialize列の読めるpageだけを数え、ZIPも同様、PDFはpage_num+1/count。クライアント入力値は検証で書き換えられる (`src/remote_ipc/container.rs:2859`, `src/remote_ipc/container.rs:3468`, `src/remote_ipc/container.rs:3695`, `src/remote_ipc/container.rs:3748`, `src/remote_ipc/container.rs:7114`)。

ordinal/totalは既に得られるが、Remoteの閲覧directionは別のcontainer payload/settings経路にあり、今のwrite handoffの検証済みtupleに無い。本件でdirection再解決・session state・wire fieldを足さず、**Remote producerはraw indexを従来どおり記録し、追加3列は全てNULL、mapの当該keyの値はNone**とする (`src/remote_ipc/ui.rs:2754`)。件数には行を含めるがメーターは描かない。本体の無関係な `App.reading_direction` や過去mapのrtlを流用しない。Remoteで読んだ本のmeterはローカルで再び読むまで消える、という合意済みの見え方を採用する。非root ZIP等のrecord_resume=falseでは従来どおり保存せず、過去root行を消さない。

本体egui一覧だけの描画変更であり、IPC wire/Web DOM/CSS/HTTP thumbnailにmeterを追加しない。**protocol 63を維持** (`crates/remote-ipc/src/lib.rs:31`)。Remoteのraw復元・履歴・bookmarkは既存動作を保ち、remote-webにDB writerを作らない。NULL対応の共通record入口へ接続するcore内部変更だけ。

## 7. 失敗の割り切り・未決事項

### 合意済みのまれな競合: 通常削除に待機を足さない

条件は、ページ記録の直後、writerが未処理の短い間にその本を通常削除した場合。purge後にRecordが旧keyへcommitされ、存在しないpathのbook_resume行が1行残ることがある。削除時にmapの該当key/配下を消すため現在の一覧には表示しない。同じpathに後で別の本ができたときだけ古い値が表示され得る (再起動等の全行再読込を含む)。同名pathの再作成に古い位置が当たるのは既存の位置復元にも元からある性質であり、利用者はこの条件・見え方を了承済み。通常削除への共通待機拡張は不採用とし、未開始削除要求を保持する状態・監視・再試行を足さない。通常削除完了時はscope除去だけ行い、そのための全行再読込はしない。

既存待機があるrename / purge retry / 明示整理等にはBookResumeWriterの処理済み待機を加える。順序テストもこの経路を対象とする。

### 合意済みのまれな競合: 内容identity復元に新しい延期状態を足さない

条件は、復元元のページRecordが未処理の数ミリ秒の間に、利用者が確認ウィンドウの復元ボタンを押して転記が始まる場合。転記が先に旧行を読めばコピー先には古いordinal/total/rtl (旧NULLならメーター無し) が入り、後から元keyの最新Recordがcommitしてもコピー先は更新されない。raw位置も同じ旧行からコピーされる。完了後のmap再読込はDBの確定値を採用するため、この古い値も一覧に出る。コピー先をローカルで開いて記録すれば最新値に置き換わる。既存のraw位置転記にも同じ競合があり、今回だけのメーター監視・再転記を加えない。

§4.2のとおり既存の延期機構がないため、利用者指定の割り切りを採用する。確認ウィンドウで利用者がボタンを押してから始まる復元が記録直後の短い時間に重なることは実用上まれであり、未開始要求・追加状態機械・専用barrierは作らない。したがって「paused Recordから本番復元入口を通し、解除後に最新値がコピーされる」という順序保証も追加せず、その保証を前提とする回帰テストは作らない。既存待機があるrename / purge retryの順序テストは維持する。

利用者の上記判断は確定として扱い、再承認を求めない。移行/初回SELECTが失敗したらログ、メーターを不表示、raw復元を維持。範囲/監視の再試行や復旧機械を作らない。通常の未読/NULL/0枚でnoticeを出さない。

DB書込失敗・稀な再読込失敗については次を設計責任者/利用者判断事項として残す。実装のためにretry/journalを設計し始めない。

- 記録のDB書込が失敗したとき: 初版は既存writerのログを維持し、mapには受付済みの最新値を残す。画面は今回の値を表示できるが、DB再読込・再起動後は保存済みの旧値/不表示へ戻る。raw位置も保存に失敗する既存条件であり、データを削除しない。追加noticeが必要なら利用者判断事項とし、そのための通知commandやretryは今回増やさない。
- rename/copy等の全行再読込が失敗したとき: 推奨は影響scopeのmeterを非表示のままにし、ログと再起動案内。map全体の信頼を失うストア置換なら全meter非表示。他scopeは継続可能。読書位置を初期化しない。

通常の操作でmap更新を取りこぼさない接続と、読込snapshotが最新記録/Clearを上書きしない§4.3は通常の正しさとして実装する。追加の復旧対応が要るまれな組合せは条件と見え方を追記して相談する。独立レビューは完了し、P2の決定は反映済み。通常削除の判断も確定し、同じ実装セッションで続行する。

## 8. テスト計画・ドキュメント更新・実装時のgate

| 対象 | 検証内容 |
| --- | --- |
| 移行/後方互換 | 出荷済2列DBに3列追加、旧path/page保持・NULL、再起動の冪等性、新DB、移行失敗でraw記録/復元維持。旧版SELECT/page-only upsertが動き、新列が残る了承済み挙動。新版NULL保存は3列を消す |
| 記録/HUD共有 | helperとHUDの列/位置が同じ。Image/Video/Folder/ZipImage/PdfPageの混在、filter・通常/詳細sort・stack flat・本扱いOFF・製本順。raw idxとordinalの違い、anchor無し、0/1枚・途中・最後・後戻り、RTL Single、見開き/連結/Split/補助表紙。HUDに相手pageが出てもanchorだけ記録 |
| 記録更新 | raw/3列の同時upsert、同idxでもtotal/ordinal/rtl/NULLが変わればdedupしない。読んで一覧へ戻るとmapが即時最新、OFFでも記録維持。内容/並びが変わっても読み直し前は保存値のまま |
| map初回/再読込 | 初回worker全行read、NULL/不正値非表示、読込中のSet/Remove/Clear/RemoveScopeを古いsnapshotが上書きしない。再読込receiver差替え、起動回復、別viewer記録で共通map更新・他context不変 |
| 他のDB変更 | Remoteで当該keyの値をNoneへ更新、非記録Remoteは不変。delete/rename/move/copy/内容identity復元のproduction DB結果とmapの一致。prefix境界、コピー先既存行優先、Clear後の再記録、DB再接続/reset。無関係なscopeを消さない |
| writerとpath-key操作の順序 | Recordをworkerで決定的に未処理のまま保持してrename / purge retryを開始。開始predicateが待機し、Record commit後に操作が始まり、旧行が復活せず新行が最新となること。sleepの偶然やFIFO単体だけで検証しない。通常purgeは§7の許容競合とmap除去だけを確認 |
| 対象セル/比率 | Folder/ZipFile/PdfFileだけ。変換書庫/Stack/個別page等は無し、旧NULL/行無し/total0はtrackも無し。1/N・途中・N/N、FollowReading×保存rtl、LeftToRight、方向設定変更は再read無し |
| layout/snapshot | meterとbadge rect非交差 (右下filter件数を含む)、極小cellは既存badge保持/meter省略、cell_h/scroll/sort/hit-test不変。Light/Dark・最小幅/普通幅・DPI・選択/check・編集/tag/pin・形式/長名/評価/filter件数、cut opacity、動画長さ回帰。純fixtureとPreferences説明をsnapshot |
| 設定の通し | 実PreferencesStateで編集→本番OK helper→保存→DB再load→開き直し。ON/OFF・方向保持、Cancel不変、欠落field/default/resetはON、favorite/runtime列設定mergeを破壊しない |
| 速度/Remote | 通常paintからDB/FS/列挙へ到達しない。可視key memoを別idxへ誤流用しない。scrollしても全行SELECT/worker/thread/watchが増えず、idle repaint loop無し。IPC63/wire fixture・Remote raw復元/履歴/bookmark不変 |

実装時に更新する文書は `docs/spec.md` と `htdocs/mimageviewer/` の設定/読書位置説明 (過去に保存した比率、旧NULL、内容変更、Remote後の非表示。版番号・内部用語を使わない)、`docs/architecture-overview.md` のbook_resume列/メモリmap、`docs/async-architecture.md` の既存writer全行read、`docs/display-pipeline.md` の下端帯、`docs/README.md` の索引、本設計の実装記録。backlog §1.256は実装完了時に「実装済み (レビュー前)」を1行加え、§2.2は今回の帯追加と現行レーンの状況を整合させる。実装・文書更新・自動検証の結果を本書へ追記する。

実装後は `cargo fmt`、`cargo check -p mimageviewer --bin mimageviewer-core`、関連 `cargo test -p mimageviewer --lib <filter>`、`cargo test --test ui_snapshot`、`python scripts/check_ui_glyphs.py` を実行し、snapshot更新は `docs/ui-snapshot-policy.md` に従う。共有変更の最終gateは `scripts/test-full.ps1` と `cargo fmt --check`。その後 `scripts/build-dev.ps1` で利用者用binaryを作り、通常profileの注意と具体的確認手順を渡す。アプリは起動しない。

将来の利用者確認は混在Folder/PDF/ZIP・途中/最後/後戻り・左右方向/見開き・読んで戻る・ON/OFF・設定保存・内容変更後の保持・Remote後の非表示。agentによるlive確認を実施するなら、disposable portableと時間/desktop操作の範囲を提示して明示承認後だけ行う。通常profile binaryはagentが起動しない。

## 9. 実装記録・検証証跡 (2026-10-04)

§1のfile:lineは実装前の調査記録として保持する。以下は実装後の参照である。

- `src/book_resume_db.rs:139` のtransactionで追加3列を移行し、同じwriterのRecord / Read / ClearをFIFOで処理する。`is_busy` はenqueueからcommand処理完了までを数える。既存UIのopen/getにALTERやメーターSELECTを追加していない。
- `src/ui_fullscreen.rs:16055` のHUD用 `image_reading_position` を `src/app.rs:50854` の記録で共有する。見開きanchor、混在列、RTL Singleを既存読み順のまま扱う。
- `src/app/book_resume_meter.rs:112` にローカル/Remoteの共通記録と即時map更新を集約した。NULLも件数用に保持する。読込後の差分をsnapshotに適用し、receiver差替えで古い読込を捨てる。path自身をmemo keyにしてviewer/idxの世代を増やしていない。
- `src/app.rs:40458` の既存待機predicateにwriterを追加した。通常削除は `:38721` のscope除去だけ、rename移行完了 `:40759` / purge retry完了 `:41009` / 明示整理 `src/ui_dialogs/metadata_cleanup.rs:64` / 内容identityの転記 `src/app/content_identity_restore.rs:416` はworker再読込へ接続した。通常file copyと設定リセット/復元はresume DBを変更しないため再読込しない。
- `src/thumb_overlay_layout.rs:477` は右下badgeのcell基準にも予約を反映する。極小cellは既存badgeを維持してmeter無し。`os_theme` のpalette、保存RTLと既存seek方向、cut opacityをpaint helperから使う。
- `src/settings.rs:4437` と `src/ui_dialogs/preferences/pages.rs:1609` に既定ONの全体設定を追加した。実PreferencesState、本番OK helper、save、settings.db再読込、開き直し、Cancelを通すテストを追加した。
- Remoteは共通記録へ補助値Noneを渡して3列をNULLにする。非root ZIPの非記録入口は旧root行を維持する。`crates/remote-ipc/src/lib.rs:31` のprotocol 63とwireは変更していない。

変更ファイル (この作業の差分):

- 永続化/共有map/経路: `src/book_resume_db.rs`, `src/app.rs`, 新規 `src/app/book_resume_meter.rs`, `src/app/content_identity_restore.rs`, `src/remote_ipc/ui.rs`, `src/ui_dialogs/metadata_cleanup.rs`, `src/ui_fullscreen.rs`。
- 描画/設定: `src/app/grid_paint.rs`, `src/os_theme.rs`, `src/thumb_overlay_layout.rs`, `src/ui_main.rs`, `src/settings.rs`, `src/ui_dialogs/preferences.rs`, `src/ui_dialogs/preferences/pages.rs`, `src/ui_dialogs/preferences/search_index.rs`, `src/lib.rs`。新項目は既存の環境設定検索索引にも登録し、既存の全anchor照合テストで確認する。
- テスト: 新規 `src/app/book_resume_meter_tests.rs`, 既存 `src/app/tests.rs`, `tests/ui_snapshot.rs`。新規PNGは `book_resume_meter_light`, `book_resume_meter_dark`, `book_resume_meter_dark_high_dpi`, `preferences_book_resume_meter_light`, `preferences_book_resume_meter_dark`。既存 `preferences_favorite_view_state_dark.png` は同じサムネイル設定ページに項目を足した結果のscrollbar端4 pixelだけを更新。6枚とも実物を目視確認した。
- 文書: `docs/README.md`, `docs/architecture-overview.md`, `docs/async-architecture.md`, 本書, `docs/display-pipeline.md`, `docs/next-release-backlog.md`, `docs/spec.md`, `htdocs/mimageviewer/manual/settings.html`, `htdocs/mimageviewer/manual/grid.html`, `htdocs/mimageviewer/manual/tut-reading.html`。

独立実装レビュー (`gpt-6.1-sol` / `xhigh`) は合成rootと物理子フォルダの対象判定を指摘し、入口によらず既存のtyped surface/positionとinstalled item flagsで判定するよう修正した。再読込でraw位置の直近記録を失わないことも回帰テストを追加した。修正後のレビューで未解決P1/P2なし。通常削除の許容競合は§7の利用者決定を保持する。backlogの状態は指定どおり「実装済み (レビュー前)」とする (設計担当の受入前)。

自動検証の結果 (ログはworktreeの `target/book-resume-*.log`、すべて非対話):

| コマンド | exit / 結果 |
| --- | --- |
| `cargo fmt` / `cargo fmt --check` | 0 / 0 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0。Box化・検索索引追加後の最終通常featureで再確認済み |
| `cargo test -p mimageviewer --lib book_resume` | 0、30 passed。移行・後方互換・HUD/RTL/anchor・map差分・記録/削除/Clear・paused Record→rename/purge retry・設定OK/save/DB再読込/Cancelを含む |
| `cargo test -p mimageviewer --lib remote_static_page_progress_is_the_next_pc_open_position` | 0、1 passed。Remote後のNULL・map非表示・raw復元 |
| `cargo test -p mimageviewer --lib remote_nested_page_without_resume_preserves_previous_root_meter` | 0、1 passed。非記録Remoteは外側root行を維持 |
| `cargo test -p mimageviewer --lib thumb_overlay_layout` | 0、20 passed。右下予約・全badge非交差・極小cell優先・duration回帰 |
| `cargo test -p mimageviewer --lib preferences::search_index` | 0、11 passed。全anchor登録・実ラベル/ページとの一致 |
| `cargo test --test ui_snapshot` | 0、61 passed。新設定2枚と既存scrollbar1枚だけ指定filterで更新、通常比較で全件確認 |
| `python scripts/check_ui_glyphs.py` | 0、dangerous glyphsなし |
| `scripts/test-full.ps1 -SuppressCrashDialogs` | 101。launcher buildのEffeTune承認チェックで停止。既存vendorはv0.12.0、追跡manifestはv0.11.1。vendor/manifestは変更せず、ゲートを迂回して成功扱いにしない |
| `cargo test --manifest-path vendor/egui/Cargo.toml --lib` | 0、25 passed |
| `cargo test --manifest-path vendor/egui-wgpu/Cargo.toml --features winit --lib` | 0、9 passed |
| `cargo test --manifest-path vendor/eframe/Cargo.toml --no-default-features --features wgpu --lib` | 0、16 passed |
| `cargo test -p mimageviewer --features pack-build-tools --test search_metadata_e2e -- --test-threads=1` | 0、14 passed。並列実行時の初期scan待ち8件の10s timeoutを個別再確認 |

全workspaceのlauncher除外実行の初回は101: 新規所有者inlineによるAppサイズ110080 (>既存上限)、未登録の検索anchor、およびlibのSTATUS_STACK_OVERFLOWを確認した。所有者をBoxへ移し、検索索引を登録した。既存サイズ上限/テストの制限は変えていない。検索E2Eの初期scan待ち8件もtimeoutしたが、単独・直列では全14件成功した。E2EはApp/book-resumeを構築せず、該当indexer/fixtureに今回の差分は無い。各managerは64MiB Tantivy writerと複数workerを持つため並列資源競合が有力だが、詰まった段階まではログで断定できない。

最終ソースの `cargo test --workspace --exclude mimageviewer-launcher --features pack-build-tools --no-fail-fast -- --test-threads=4` は101。libは10220 passed / 1 failed / 51 ignoredで、失敗は `ui_dialogs::about::tests::effetune_notice_texts_include_attribution_and_match_vendor_when_present` の既存vendor版確認だけ (`src/ui_dialogs/about.rs:311`: actual v0.12.0 / expected v0.11.1)。他のworkspace targetは全て成功し、検索E2E14件、UI snapshot61件も成功した。App stack上限、検索anchor、capture/fullscreenを含む他のlib回帰は通り、STATUS_STACK_OVERFLOWは再発していない。Appサイズは109936 bytes。元の `test-full.ps1` が通ったという意味には置き換えない。

上記は前回検証の記録であり、当時の全ゲート条件は既存EffeTune入力と追跡済みv0.11.1承認境界の不一致だった。今回の機能差分でvendorの差し替え、notice/manifest更新、testの版確認解除は行っていない。表の指定通常featureコマンドはBox化と検索索引追加後の最終ソースでも全て0を確認した。

確認用ビルドは `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 1` がexit 0。core / Remote service / EPUB PDF workerを `target/dev-runtime` に作成し、FFmpegとVC runtimeを配置した。VC runtimeのPE検証も成功 (runtime=4 / pe=3)。`-PreserveRuntime` により常駐製品を停止せず、製品binaryを起動していない。ログは `target/book-resume-build-dev.log`。既存vendorのEffeTune v0.12.0を使用した確認用coreのビルド成功であり、上記のrelease承認境界の不一致が解決したという意味ではない。

利用者の手動確認は、インストール済み/トレイ常駐のmImageViewerを終了してから `Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。既定では通常の `%APPDATA%\mimageviewer` を使い、実際の設定/データを更新し得る。Folder / ZIP / PDFを途中・最後まで読んで一覧へ戻る、保存RTLと方向設定の組合せ、表示ON/OFFのOK保存と開き直し、狭いセルの右下badgeとの非交差を確認する。agentによるアプリ起動・commitは行っていない。

### 後続独立レビューへの対応・EffeTune差し替え後の再検証

内容identity復元P2は、§4.2で開始/延期ownerを確認し、利用者の条件付き決定に従って§7の割り切りを採用した。製品コード・状態・テストの追加はなく、設計書とasync文書の適用範囲を訂正した。既存待機経路の順序保証と復元完了後のmap更新は変更していない。

利用者が `vendor/effetune-mixwright` を承認済みv0.11.1へ差し替えた後、次を実行した。環境は当該未コミット差分、`CARGO_BUILD_JOBS=1`、全体gateのみ `RUST_TEST_THREADS=4` (以前の資源競合を避けるため)。

| コマンド | exit / 件数・結果 |
| --- | --- |
| `cargo test -p mimageviewer --lib ui_dialogs::about` | 0、2 passed / 0 failed / 0 ignored。EffeTune attribution/版一致を含む。ログ `target/book-resume-about-retest.log` |
| `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\test-full.ps1 -SuppressCrashDialogs` | 101、テスト実行前のbuild停止 (実行件数0)。EffeTune承認エラーは出ず、launcher入力の `target/release/mimageviewer-core.exe` / `mimageviewer-remote.exe` / `mimageviewer-epub-pdf.exe` が未作成。ログ `target/book-resume-test-full-retest.log` |

現在の未解決gate条件はrelease版の上記3入力を準備して全体gateを完走すること。前回の10220 passed / 1 failedは履歴であり、今回の全体成功件数として足し合わせない。確認用dev-runtime成果物は前回のEffeTune v0.12.0を配置したものなので、今回のv0.11.1差し替え済み配布物としては扱わない。今回は文書修正と指定の再検証だけで確認用buildの再作成はしていない。製品binaryの起動・commitなし。
