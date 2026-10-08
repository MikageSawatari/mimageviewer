# §1.256 一覧の本サムネイルの読書位置メーター — 記録値を表示する設計

現行の配置方針は、共有設定ONで全セルに固定下端帯を予約し、下端ラベルを一律13 logical pt上へ移す方式 (§5.1 / §14)。代表画像なし・未ロードの検索セルの多行階層パスは上端を元位置に保ち、下端だけ13pt縮めて末端名を優先する。代表画像ありは背景/パスの全体を13pt移動する方式を維持する (§14.4)。帯はセルの左右4pt内側の幅・高さ9 logical ptで、位置記録の有無や媒体種別によって位置・厚さを変えない。通常コピー/移動は既存再生位置と同じく対象外とする利用者決定を維持する。§10〜§13と§14.3の実装・検証記録は各修正時点の履歴として保持し、後続修正の成功証跡には流用しない。

作成・改訂: 2026-10-04。コード調査基準: `next-file-ops` / `e804db069`。
2026-10-07追記: §1.350のRAR/CBR/7z/CB7/LZH/LHAセル対応は§15。初版の除外契約を§2で更新した。当時の検証・レビュー記録は履歴として保持する。2026-10-08の実機確認後の決定（キャッシュ削除後もバーを保持）は§18。同日の実機確認で見つかった公開済み分割RAR開封経路の不具合（§1.355）は§19。
状態: **固定下端予約帯への改訂は検証・確認用ビルド済み。後続P2の検索セル名消失も修正済み (§14.4)、修正後gate・独立レビュー・確認用ビルドは完了。利用者の実機確認待ち。§12/§13と§14.3の成功記録は各修正時点の結果。** 保存・watched・常に左→右の仕様は維持する。独立設計レビュー (`gpt-6.1-sol` / `xhigh`) のP2 2件と2026-10-04の設計担当決定を反映済み。後続独立レビューの内容identity復元P2も、既存延期機構がないため利用者指定の割り切りで対応 (§4.2 / §7 / §9)。通常削除の競合も利用者合意済み。commit・アプリ起動は行っていない。
要件: [next-release-backlog.md §1.256](next-release-backlog.md#1256-一覧の本サムネイルに前回読んだ位置のメーターを表示する--438-2026-09-19)。本書の仕様判断は、2026-10-04の利用者合意によって以前の厳密な内容照合案を置き換える。file:line は調査基準時点のコード事実、追加する型・列・APIは提案である。

## 0. 合意した設計と前提

**実装前提の矛盾 (2026-10-04確認): 通常の削除には「移行前にwriterの処理済みを待つ」経路がない。** `rename_migration_writers_busy` (`src/app.rs:40444`) はrename開始 (`:40507`)、purge retry開始 (`:41034`)、明示メタデータ整理 (`src/ui_dialogs/metadata_cleanup.rs:108`)、明示メタデータ転記 (`src/ui_dialogs/metadata_transfer.rs:303`) に使われる。このpredicateにBookResumeWriterを加える決定は実施可能。一方、通常削除の `start_delete_files` (`src/app.rs:41057`) はjournalのadmissionだけを確認 (`:41069`) し、writer待機なしにdelete workerをspawnする。workerはShell削除後、末尾で直接purge (`src/delete_worker.rs:298`) する。`acquire_delete_epub_guard` (`:203`) はEPUBの範囲保護で、BookResumeWriterのRecord完了を待つものではない。このためpredicateへの追加だけでは「未処理Record → 通常delete/purge → 旧keyへのRecord commit」の復活を防げない。

2026-10-04の追加決定: 通常削除への共通待機拡張は採用しない。短い競合で存在しないpathの行が残ることを許容し、削除時にはmapの該当scopeを消す。rename / purge retryなど既存の待機経路だけにBookResumeWriterを加える。新しい未開始削除要求・専用barrier・UI待機を作らない (§7)。

**記録時の「何ページ目 / 全何ページ」を保存し、一覧はその値を表示する。** 読み順や中身を後から数え直さない。起動時の全行読込と稀なDB変更後の再読込を既存writerで行い、通常のスクロール・描画はメモリmap参照だけにする。

利用者了承済みの割り切り: 旧行の追加列がNULLなら次に読むまで不表示。読んだ後に内容や並びが変わっても次の記録までは保存した比率を表示。旧版へ戻して読んだときに新列の古い値が残っても許容する。移行失敗はログとメーター非表示で扱い、従来の位置復元は維持する。proof JSON、trigger、内容版・manifest、監視、未開封本の方向解決、可視範囲worker、これらに付随する世代管理は採用しない。

前提のコード上の注意: 保存済み `page` はページ番号ではなくraw `items` index。HUDには要求どおりの読めるページ列を使う計算があり (§1.5)、これを再利用できる。Remoteは既存のNULL保存を維持する (§6)。backlog §2.2 の「右上/右下未着手」は現行layoutと異なるため、現行 `ThumbnailOverlayLayout` を基準に下端の帯だけを足す (§5)。記録値の表示方式は実施可能で、通常削除の順序保証は上記の割り切りで判断済み。

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

保存する補助値は `ReadingMeterValue { ordinal, total }`。`ordinal` はHUDと同じ読めるページ列での1-based位置、`total` はその列の長さ。塗る方向は常に左から右とし、読み方向を保存しない。追加DB readをしない。

raw `page` と復元処理は変更しない。補助値は実際の読書時点のsnapshotであり、「現在の内容ならraw indexがどのページへ復元されるか」を保証する値ではない。読み順のfilter・sortに従って分母も変わる。親の一覧を絞り込み/並べ替えただけでは保存値を書き換えない。戻って読み直すとordinalも比率も減る。最大到達ページ・読了判定ではない。

見開きは記録されるanchorで数える。2–3ページでanchor=2なら2/N。最終見開きでもanchor=N−1なら(N−1)/N、anchor=Nなら100%。相手ページを推測して+1しない。添えた表紙・白slot・Splitの左右半面を増分として数えない。1ページ本の有効記録は1/1。

| 一覧セル/閲覧種別 | 現行の扱い（§1.350の対象拡張を含む） |
| --- | --- |
| 通常一覧のFolder | 対象。本扱いON/OFFを問わない。画像だけでも、動画・子フォルダ・非画像が混ざっていても、記録時HUDの読めるページだけで数える |
| 製本フォルダのFolder | 同じmap参照で対象。製本の実際の読み順で記録する。追加・並べ替え後も再記録までは保存値を表示 |
| ZipFile / PdfFile | 当該実パスのresume keyでmapを引いて表示。ページ数列/catalog/passwordの再取得をしない |
| ZIPのroot / 単一wrapper root | 現状記録する範囲を維持。rootにZipDirが混じっていても、記録時に送り得るZipImageだけで数える |
| 入れ子ZIP内側 | 現状 `record_book_resume` が記録しないため対象外。その閲覧で外側rootの過去記録を消す処理も足さない |
| Stackセル / Image / ZipImage / PdfPage / ZipDir個別セル | メーターを描かない。flat stack閲覧が従来記録する値はHUDの読み順で補助値も記録でき、後の通常Folderセルに表示される。stack専用keyを新設しない |
| ConvertibleArchive (直接閲覧RARを含む) | 既存の非同期`converted_archive_cache_paths`を使う。Directは元書庫key、CachedZipは実読込path、論理sourceが確定したUnavailableだけは論理sourceから純粋計算した決定的な変換ZIP keyで同じmapを参照。キャッシュ削除後もバーを表示する。Pending / 未登録 / 論理source未確定では描かない。分割RARはheaderで確認した先頭volumeを使用し、ファイル名による推測や描画中I/Oはしない。保存・復元keyは維持（§18） |
| PdfFile扱いのEPUB | resume保存keyと当該cell pathが一致して行があれば同じmap参照で表示。変換generation/内容を解き直さず、異なるkeyを推測で結ばない |
| 詳細行・seek strip・Remote Web一覧・合成ビュー専用表示 | メーター描画は対象外。通常物理一覧のFolder/ZipFile/PdfFile/ConvertibleArchiveセルに限定。Tag/Smart/Collection等から入った物理子フォルダも入口を問わず対象、合成rootはメーター非対象 (既存surface/positionとinstalled itemflagsを参照)。PCのgridセルの帯予約と下端caption移動は§5.1の全セル規則に従う |

行無し、追加列が1つでもNULL、total==0、不正値 (ordinal<=0 / ordinal>total) ではtrackも含め描かない。0%への代用やclampはしない。既に保存された有効値は、内容の変更・外部削除・password状態・認識規則変更等と再照合しない。通常の一覧更新によりcellが消えると描画も消えるだけで、本ごとの監視は不要。

## 3. 永続化・記録入口

### 3.1 追加列とwriter起動時の移行

リリース済み確認: `CHANGELOG.md:599` のv1.1.0内 `:604` に読書位置復元、`git show v2.6.0:src/book_resume_db.rs` に既存2列表を確認。旧path/pageを保持し、writer起動時に次のnullable列を足す。

```sql
ALTER TABLE book_resume ADD COLUMN page_ordinal INTEGER;
ALTER TABLE book_resume ADD COLUMN page_total INTEGER;
```

2026-10-04の方向変更: 元の `path/page` はリリース済みであり、旧2列DBからの移行を維持する。一方、メーターの追加列はこのブランチの未リリース変更なので、CLAUDE.md「永続データ・スキーマ変更時の判断」の未リリース扱いでordinal/totalの2列追加へ変更する。以前の3列追加版 (`reading_rtl` を含む) の開発用DBへの移行・列削除・互換分岐は作らない。実データをagentが削除することもない。

既存writer自身の接続でPRAGMA table_infoを見て不足列だけ追加する。2列追加は1transactionで行い、旧行はNULL、新DBも同じ経路を使う。UI側の既存open/getにALTERや追加列照会を足さない。移行成功後に初回全行読込を行う。移行失敗ならログ、メーター利用不能とし、従来2列へのraw記録/SELECTと復元を継続する。読書位置を削除・初期化しない。trigger、JSON、schema専用journal、再試行loopを作らない。旧版のpage-only upsertで新列の古い値が残るのは了承済み。

### 3.2 ローカルの記録

`record_book_resume` の既存画像種別/ZIP root条件を維持し、§1.5の共通計算からordinal/totalを取得する。方向は保存しない。計算できなければ補助値無しとする。追加scan、read_dir、PDF/ZIP列挙、page-count DB readを行わない。HUDと同じcached列を参照し、必要なposition検索は記録時だけ行う。ページ送り毎の追加全列clone/別のfilter処理を作らない。

writerへ `path / raw_idx / Option<ReadingMeterValue>` を送り、UI側のmapも同じ共通入口で更新する。既存 `last_book_resume` のdedupをpath/raw_idxだけで済ませず、補助値を含むrecord全体で比較する。同じidxでも読み順の分母・ordinal、NULL→有効値が変われば更新する。directionだけの変更は保存値を変更しない。ordinal/totalはusizeからSQLite INTEGERへのchecked変換を行い、表現できない値を丸めず補助値無しで記録する。

通常モードのwriterは1回のupsertでraw pageと追加2列を同時保存する。補助値無しなら2列ともNULLに上書きし、旧meterを残さない。未移行モードのwriterは従来pageだけを書き、メーターは描画しない。書込を受け付けた最新値をmapへ即時反映するため、一覧へ戻るとDB commitを待たずに直近の位置が出る。書込失敗の割り切りは§7。

## 4. 一覧用メモリmapと稀な更新

### 4.1 所有と初回読込

App全体が正規化keyから保存補助値Optionへのmapを1つ持つ。mapと読込受付の所有者はBoxに置き、既存Appのstackサイズ上限を維持する (状態・所有・処理経路は不変)。NULL/不正値行はNoneとして保持し、表示しないが登録件数をmap.lenで求める (UIでCOUNTしない)。初回読込前/初期化失敗はmapのOptionがNone、利用可能時はSomeとし、別の準備完了/失敗boolを足さない。main/detachedのitemsやnavigation cacheにmapを複製しない。各viewerで記録する共通入口が同じmapを更新する。キーは既存book_resumeと同じDriveStripped正規化で、別ドライブ同名pathの衝突も現行仕様を引き継ぐ。

既存 `BookResumeWriter` に全行読込command/結果を加え、新規workerを作らない。起動時は移行後にSELECTを1回行い、追加2列が有効な行のmapを構築してUIへ返す。初回のmapがまだ無くても一覧は通常表示し、到着後にrepaintする。初回待機中の記録は§4.3の差分へ保持し、移行/初回SELECT成功後のsnapshotに反映して初めて描く。初期化失敗ではNoneのままで、raw記録は続けてもmeterだけは出さない。UI側はDBを読まない。表示OFFでも起動時の1回読込と記録時更新を行い、ONへ戻す際の追加loadを不要にする。

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

rename/copyの全列移行は既存store registryを使用し、追加2列も一緒に運ぶ。稀な操作後の全行SELECTは許容し、独自prefix移行SQLや新しい差分DB監視は作らない。移行中は影響scopeの古いcell値を出さず、結果でrepaintする。他viewerのitems/worker/選択は変更しない。起動時の既存移行回復・後続cleanup完了も同じ再読込入口へ接続する。

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

### 5.1 全セルに固定下端帯を予約する配置 (2026-10-06再改訂)

`layout_cell_overlays` は `rect.shrink(4)` のinnerと実測badgeを純layoutへ渡し、`draw_cell` は同じlayoutをcell内へclipする。現行 `ThumbnailOverlayLayout` はcheck/stack/top-left/bottom-left/filter count/media durationを所有する。ラベルの配置とバーの配置を同じ描画経路で解決し、backlog §2.2の古い未着手記録を理由に四隅を再実装しない。

共有設定 `thumb_show_resume_meter` がONなら、読書/再生位置がないセルも含め、本・動画・音声・画像・その他の全セルに同じ下端帯を予約する。layoutへ渡すboolはこの設定そのものとし、`fraction.is_some()` に置き換えない。有効な比率の有無は予約後のバー描画だけを決める。OFFなら予約せず従来の下端ラベル位置へ戻す。同じ設定状態では、記録の有無や媒体種別で帯・ラベルのgeometryを変えない。

帯の横範囲は `inner = cell.shrink(4)` の全幅、下端は `inner.max.y` (セル下端から4pt)、高さは9 logical ptで固定する。画像幅・縦横比・fit・回転に合わせない。描画時は左端をphysical pixelのceil、右端・下端をfloorへ揃え、上端を丸めた下端から `floor(9 * pixels_per_point)` pixels戻して決める。上端まで元の矩形の内側に丸める規則ではない。100/125/150/200%の描画厚さは9/11/13/18px (9/8.8/8.667/9 logical pt)。角丸の不透明バーをこの矩形へ描き、常に左から右へ塗る。位置のないセルはtrackも描かず、予約した空き領域だけが残る。

ONでは下端ラベルを左右とも一律13 logical pt上へ移す。右下の既存基準が `cell.max.y - 3`、帯下端が `cell.max.y - 4` なので、9ptの帯と最低3ptのgapを確保する移動量は `9 + 3 + (4 - 3) = 13`。右下ラベルと帯上端のgapは3pt。左下の既存基準は `inner.max.y - 3` なので同じ13pt移動でgapは7ptとなる。ファイル名・形式・フォルダ名・評価と、右下の長さ/絞り込み件数を同じ予約量で配置する。純layout外のSearchContainerの件数とCollectionPlaceholderの単行captionも13pt移動する。SearchContainerの代表画像なし・未ロードの多行階層パスだけ、下記の専用規則で高さを13pt減らす。極小セルで下端badgeが入らない場合は既存の低優先省略を使い、帯の位置・厚さを変えない。clipは既存のcell境界を維持する。

SearchContainerの代表画像なし・未ロードの階層パスは、ONでも矩形の上端を固定アイコン下の元の位置に保ち、下端だけを13pt縮める。縮めた矩形で `ui_helpers::layout_path_hierarchy` の既存計算を再実行し、入りきらない親階層から省略して末端の本/フォルダ名を優先する。この経路で階層パス全体を上へ移して固定アイコンと交差させ、その全体を非表示にする方式は採用しない。代表画像ありでは、もともと小さい固定種別アイコンより下に背景/パスがあるため、背景/パス全体を13pt上へ移す従来方式を維持し、高さを縮めない。通常サイズのセルで末端名が丸ごと消えることを許さない。

SearchContainerの件数やCollectionPlaceholderの単行captionは、ONのときだけ描画と同じpixel原点へ丸めたgalleyの実際のinkを確認し、cellに収まらない、帯に交差する、固定の主アイコンに交差する場合は低優先captionを省略する。別の位置を探索せず、バー・主アイコンを移動しない。収まりの判定はcell境界を使い、viewportのclip境界とは分離するため、スクロールで一部だけ見えるcaptionを丸ごと消さない。SearchContainerの代表画像ありのラベル背景はONなら元の上部アイコンより先に描き、極小セルで背景が主アイコンを覆わないようにする。OFFは元の描画順を維持する。階層パスの既存計算を共有するため `layout_path_hierarchy` の可視性を `pub(crate)` とする。

cell_h・cell rect・並び順・画像fit・中央の再生/音楽アイコンの位置/大きさ・scroll contentは不変。タグhit-testは移動後の同じBadgePlacementを使う。回転/補正済bitmapやcatalog thumbnailに焼き込まない。通常セルの描画順はサムネイル/内容→バー→媒体アイコン/切り取りマーク→通常ラベル→選択border/見開き相手cursorの枠→check。固定予約帯とラベルは交差しないため、バーをラベルより先に描ける。ONで実際にバーがあるセルだけ主マークの描画をバー直後まで遅らせ、ラベルより先という従来の優先関係を保持する。OFF/位置なしでは元の描画順、cutのcontent opacityも維持する。SearchContainer / CollectionPlaceholderの特殊captionは上記の内容描画内の収まり規則に従う。バー用の空き領域を探索して上へ移す処理、実glyph/中央アイコンのobstacle追跡、画像幅への対応を撤去する。captionには上記の既存階層layout再計算と、固定位置の単行caption省略だけを使う。新しい状態・設定・worker・DB I/Oは追加しない。設定だけから全セルの配置を決め、非同期の長さ取得や位置更新をlayout変更の契機にしない。この簡素化により、専用の再構築・失効・バーの障害物再計測は不要となる。snapshotと実測は§14へ記録する。

### 5.2 常に左から右へ伸ばす

2026-10-04の利用者決定: メーターは常に左から右へ伸ばす。右綴じ・左綴じが混在する一覧の向きを揃えるため、`fullscreen_seek_direction` や本の実効読み方向とは連動させない。paint helperは方向引数を持たず、左端から `width * ordinal / total` だけ塗る。読み順での位置・見開きanchorの数え方は変えない。

比率はordinal/total。1/N、途中、N/Nをそのまま左端から描く。seek方向や本の綴じ方向を変更してもバーの方向は変えず、方向をmap/DBへ保存しない。

### 5.3 色・設定・再描画

色は `os_theme::book_resume_meter_palette(effective_dark)` 相当のsemantic helperで所有し、paint側へLight/DarkのRGB分岐を分散させない。trackとboundaryは従来の不透明灰色を維持する。採用fillはLightがRGB(38, 67, 122) / `#26437A`、DarkがRGB(142, 176, 234) / `#8EB0EA`。選択strokeのRGB(60, 120, 220)とは明度/彩度の異なる紺色・明るい青紫とし、緑のFolder badgeとも区別する。fill/trackのコントラスト比はLight 7.91、Dark 6.26、fill/選択strokeはLight 2.27、Dark 1.94 (§14)。テーマは当該UIのresolved visualsを使い、OSテーマ固定値をcacheしない。テーマ変更はpalette再取得だけ。

設定は **`thumb_show_resume_meter`、既定ON**、全体共通。環境設定 **表示 → サムネイル** に **「本・動画・音声のサムネイルに前回の位置を表示」**。説明は「メーターは常に左から右へ伸びます。記録されたページ位置を表示します。未読・位置やページ数を確認できない本には表示しません」。favorite/本別設定・方向独立設定は増やさない。閲覧表示の「ページシークバーの方向」には連動せず、以前追加したmeterへの適用説明を削除する。

既存draft編集→OK→prepare/merge→ `install_preferences_settings` → `settings.save()` を使う (`src/ui_dialogs/preferences.rs:1924`, `src/ui_dialogs/preferences.rs:1957`, `src/ui_dialogs/preferences.rs:2448`, `src/ui_dialogs/preferences.rs:2592`)。OFFはバー描画と帯予約を止め、map/記録は保持する。ONは全セルに帯を予約し、保持mapに有効な比率があれば表示する。Cancelはruntimeへ適用しない。serde欠落既定値・settings.db roundtrip・default/reset・Preferences管理フィールドとしてのmergeを揃え、既存設定確定のrepaint経路へ接続する。

## 6. Remoteの選択: 初版は追加2列をNULL保存

Remoteの `ValidatedPageContext` はraw page_index / 1-based page_number / page_count /記録可否を持つ (`src/remote_ipc/container.rs:1985`)。Folderはmaterialize列の読めるpageだけを数え、ZIPも同様、PDFはpage_num+1/count。クライアント入力値は検証で書き換えられる (`src/remote_ipc/container.rs:2859`, `src/remote_ipc/container.rs:3468`, `src/remote_ipc/container.rs:3695`, `src/remote_ipc/container.rs:3748`, `src/remote_ipc/container.rs:7114`)。

Remote producerは従来のNULL記録を維持する。今回はRemoteの検証済みtupleからHUD読み順の位置へ変換する経路を追加せず、**raw indexを従来どおり記録し、追加2列は全てNULL、mapの当該keyの値はNone**とする (`src/remote_ipc/ui.rs:2754`)。件数には行を含めるがメーターは描かない。Remoteで読んだ本のmeterはローカルで再び読むまで消える、という合意済みの見え方を採用する。非root ZIP等のrecord_resume=falseでは従来どおり保存せず、過去root行を消さない。

本体egui一覧だけの描画変更であり、IPC wire/Web DOM/CSS/HTTP thumbnailにmeterを追加しない。**protocol 63を維持** (`crates/remote-ipc/src/lib.rs:31`)。Remoteのraw復元・履歴・bookmarkは既存動作を保ち、remote-webにDB writerを作らない。NULL対応の共通record入口へ接続するcore内部変更だけ。

## 7. 失敗の割り切り・未決事項

### 合意済みのまれな競合: 通常削除に待機を足さない

条件は、ページ記録の直後、writerが未処理の短い間にその本を通常削除した場合。purge後にRecordが旧keyへcommitされ、存在しないpathのbook_resume行が1行残ることがある。削除時にmapの該当key/配下を消すため現在の一覧には表示しない。同じpathに後で別の本ができたときだけ古い値が表示され得る (再起動等の全行再読込を含む)。同名pathの再作成に古い位置が当たるのは既存の位置復元にも元からある性質であり、利用者はこの条件・見え方を了承済み。通常削除への共通待機拡張は不採用とし、未開始削除要求を保持する状態・監視・再試行を足さない。通常削除完了時はscope除去だけ行い、そのための全行再読込はしない。

既存待機があるrename / purge retry / 明示整理等にはBookResumeWriterの処理済み待機を加える。順序テストもこの経路を対象とする。

### 合意済みのまれな競合: 内容identity復元に新しい延期状態を足さない

条件は、復元元のページRecordが未処理の数ミリ秒の間に、利用者が確認ウィンドウの復元ボタンを押して転記が始まる場合。転記が先に旧行を読めばコピー先には古いordinal/total (旧NULLならメーター無し) が入り、後から元keyの最新Recordがcommitしてもコピー先は更新されない。raw位置も同じ旧行からコピーされる。完了後のmap再読込はDBの確定値を採用するため、この古い値も一覧に出る。コピー先をローカルで開いて記録すれば最新値に置き換わる。既存のraw位置転記にも同じ競合があり、今回だけのメーター監視・再転記を加えない。

§4.2のとおり既存の延期機構がないため、利用者指定の割り切りを採用する。確認ウィンドウで利用者がボタンを押してから始まる復元が記録直後の短い時間に重なることは実用上まれであり、未開始要求・追加状態機械・専用barrierは作らない。したがって「paused Recordから本番復元入口を通し、解除後に最新値がコピーされる」という順序保証も追加せず、その保証を前提とする回帰テストは作らない。既存待機があるrename / purge retryの順序テストは維持する。

利用者の上記判断は確定として扱い、再承認を求めない。移行/初回SELECTが失敗したらログ、メーターを不表示、raw復元を維持。範囲/監視の再試行や復旧機械を作らない。通常の未読/NULL/0枚でnoticeを出さない。

DB書込失敗・稀な再読込失敗については次を設計責任者/利用者判断事項として残す。実装のためにretry/journalを設計し始めない。

- 記録のDB書込が失敗したとき: 初版は既存writerのログを維持し、mapには受付済みの最新値を残す。画面は今回の値を表示できるが、DB再読込・再起動後は保存済みの旧値/不表示へ戻る。raw位置も保存に失敗する既存条件であり、データを削除しない。追加noticeが必要なら利用者判断事項とし、そのための通知commandやretryは今回増やさない。
- rename/copy等の全行再読込が失敗したとき: 推奨は影響scopeのmeterを非表示のままにし、ログと再起動案内。map全体の信頼を失うストア置換なら全meter非表示。他scopeは継続可能。読書位置を初期化しない。

通常の操作でmap更新を取りこぼさない接続と、読込snapshotが最新記録/Clearを上書きしない§4.3は通常の正しさとして実装する。追加の復旧対応が要るまれな組合せは条件と見え方を追記して相談する。独立レビューは完了し、P2の決定は反映済み。通常削除の判断も確定し、同じ実装セッションで続行する。

## 8. テスト計画・ドキュメント更新・実装時のgate

| 対象 | 検証内容 |
| --- | --- |
| 移行/後方互換 | 出荷済2列DBに2列追加、旧path/page保持・NULL、再起動の冪等性、新DB、移行失敗でraw記録/復元維持。旧版SELECT/page-only upsertが動き、新列が残る了承済み挙動。新版NULL保存は2列を消す |
| 記録/HUD共有 | helperとHUDの列/位置が同じ。Image/Video/Folder/ZipImage/PdfPageの混在、filter・通常/詳細sort・stack flat・本扱いOFF・製本順。raw idxとordinalの違い、anchor無し、0/1枚・途中・最後・後戻り、RTL Single、見開き/連結/Split/補助表紙。HUDに相手pageが出てもanchorだけ記録 |
| 記録更新 | raw/2列の同時upsert、同idxでもtotal/ordinal/NULLが変わればdedupしない。読んで一覧へ戻るとmapが即時最新、OFFでも記録維持。内容/並びが変わっても読み直し前は保存値のまま |
| map初回/再読込 | 初回worker全行read、NULL/不正値非表示、読込中のSet/Remove/Clear/RemoveScopeを古いsnapshotが上書きしない。再読込receiver差替え、起動回復、別viewer記録で共通map更新・他context不変 |
| 他のDB変更 | Remoteで当該keyの値をNoneへ更新、非記録Remoteは不変。delete/rename/move/copy/内容identity復元のproduction DB結果とmapの一致。prefix境界、コピー先既存行優先、Clear後の再記録、DB再接続/reset。無関係なscopeを消さない |
| writerとpath-key操作の順序 | Recordをworkerで決定的に未処理のまま保持してrename / purge retryを開始。開始predicateが待機し、Record commit後に操作が始まり、旧行が復活せず新行が最新となること。sleepの偶然やFIFO単体だけで検証しない。通常purgeは§7の許容競合とmap除去だけを確認 |
| 対象セル/比率 | Folder/ZipFile/PdfFileだけ。変換書庫/Stack/個別page等は無し、旧NULL/行無し/total0はtrackも無し。1/N・途中・N/N、常に左→右、読み方向/シーク方向の変更でも保存値・塗り方向は不変 |
| layout/snapshot | meterとbadge rect非交差 (右下filter件数を含む)、極小cellは既存badge保持/meter省略、cell_h/scroll/sort/hit-test不変。Light/Dark・最小幅/普通幅・DPI・選択/check・編集/tag/pin・形式/長名/評価/filter件数、cut opacity、動画長さ回帰。純fixtureとPreferences説明をsnapshot |
| 設定の通し | 実PreferencesStateで編集→本番OK helper→保存→DB再load→開き直し。ON/OFF・方向保持、Cancel不変、欠落field/default/resetはON、favorite/runtime列設定mergeを破壊しない |
| 速度/Remote | 通常paintからDB/FS/列挙へ到達しない。可視key memoを別idxへ誤流用しない。scrollしても全行SELECT/worker/thread/watchが増えず、idle repaint loop無し。IPC63/wire fixture・Remote raw復元/履歴/bookmark不変 |

実装時に更新する文書は `docs/spec.md` と `htdocs/mimageviewer/` の設定/読書位置説明 (過去に保存した比率、旧NULL、内容変更、Remote後の非表示。版番号・内部用語を使わない)、`docs/architecture-overview.md` のbook_resume列/メモリmap、`docs/async-architecture.md` の既存writer全行read、`docs/display-pipeline.md` の下端帯、`docs/README.md` の索引、本設計の実装記録。backlog §1.256は実装完了時に「実装済み (レビュー前)」を1行加え、§2.2は今回の帯追加と現行レーンの状況を整合させる。実装・文書更新・自動検証の結果を本書へ追記する。

実装後は `cargo fmt`、`cargo check -p mimageviewer --bin mimageviewer-core`、関連 `cargo test -p mimageviewer --lib <filter>`、`cargo test --test ui_snapshot`、`python scripts/check_ui_glyphs.py` を実行し、snapshot更新は `docs/ui-snapshot-policy.md` に従う。共有変更の最終gateは `scripts/test-full.ps1` と `cargo fmt --check`。その後 `scripts/build-dev.ps1` で利用者用binaryを作り、通常profileの注意と具体的確認手順を渡す。アプリは起動しない。

将来の利用者確認は混在Folder/PDF/ZIP・途中/最後/後戻り・左右方向/見開き・読んで戻る・ON/OFF・設定保存・内容変更後の保持・Remote後の非表示。agentによるlive確認を実施するなら、disposable portableと時間/desktop操作の範囲を提示して明示承認後だけ行う。通常profile binaryはagentが起動しない。

## 9. 実装記録・検証証跡 (2026-10-04)

§1のfile:lineは実装前の調査記録として保持する。以下は実装後の参照である。

- `src/book_resume_db.rs:135` のtransactionで追加2列を移行し、同じwriterのRecord / Read / ClearをFIFOで処理する。`is_busy` はenqueueからcommand処理完了までを数える。既存UIのopen/getにALTERやメーターSELECTを追加していない。
- `src/ui_fullscreen.rs:16055` のHUD用 `image_reading_position` を `src/app.rs:50865` の記録で共有する。見開きanchor、混在列、RTL Singleを既存読み順のまま扱う。
- `src/app/book_resume_meter.rs:112` にローカル/Remoteの共通記録と即時map更新を集約した。NULLも件数用に保持する。読込後の差分をsnapshotに適用し、receiver差替えで古い読込を捨てる。path自身をmemo keyにしてviewer/idxの世代を増やしていない。
- `src/app.rs:40458` の既存待機predicateにwriterを追加した。通常削除は `:38721` のscope除去だけ、rename移行完了 `:40759` / purge retry完了 `:41009` / 明示整理 `src/ui_dialogs/metadata_cleanup.rs:64` / 内容identityの転記 `src/app/content_identity_restore.rs:416` はworker再読込へ接続した。通常file copyと設定リセット/復元はresume DBを変更しないため再読込しない。
- `src/thumb_overlay_layout.rs:477` は右下badgeのcell基準にも予約を反映する。極小cellは既存badgeを維持してmeter無し。`os_theme` のpalette、常に左→右の塗り方向、cut opacityをpaint helperから使う。
- `src/settings.rs:4437` と `src/ui_dialogs/preferences/pages.rs:1609` に既定ONの全体設定を追加した。実PreferencesState、本番OK helper、save、settings.db再読込、開き直し、Cancelを通すテストを追加した。
- Remoteは共通記録へ補助値Noneを渡して2列をNULLにする。非root ZIPの非記録入口は旧root行を維持する。`crates/remote-ipc/src/lib.rs:31` のprotocol 63とwireは変更していない。

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

利用者の手動確認は、インストール済み/トレイ常駐のmImageViewerを終了してから `Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。既定では通常の `%APPDATA%\mimageviewer` を使い、実際の設定/データを更新し得る。Folder / ZIP / PDFを途中・最後まで読んで一覧へ戻る、左右綴じやシーク方向にかかわらず常に左→右の表示、表示ON/OFFのOK保存と開き直し、狭いセルの右下badgeとの非交差を確認する。agentによるアプリ起動・commitは行っていない。

### 後続独立レビューへの対応・EffeTune差し替え後の再検証

内容identity復元P2は、§4.2で開始/延期ownerを確認し、利用者の条件付き決定に従って§7の割り切りを採用した。製品コード・状態・テストの追加はなく、設計書とasync文書の適用範囲を訂正した。既存待機経路の順序保証と復元完了後のmap更新は変更していない。

利用者が `vendor/effetune-mixwright` を承認済みv0.11.1へ差し替えた後、次を実行した。環境は当該未コミット差分、`CARGO_BUILD_JOBS=1`、全体gateのみ `RUST_TEST_THREADS=4` (以前の資源競合を避けるため)。

| コマンド | exit / 件数・結果 |
| --- | --- |
| `cargo test -p mimageviewer --lib ui_dialogs::about` | 0、2 passed / 0 failed / 0 ignored。EffeTune attribution/版一致を含む。ログ `target/book-resume-about-retest.log` |
| `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\test-full.ps1 -SuppressCrashDialogs` | 101、テスト実行前のbuild停止 (実行件数0)。EffeTune承認エラーは出ず、launcher入力の `target/release/mimageviewer-core.exe` / `mimageviewer-remote.exe` / `mimageviewer-epub-pdf.exe` が未作成。ログ `target/book-resume-test-full-retest.log` |

現在の未解決gate条件はrelease版の上記3入力を準備して全体gateを完走すること。前回の10220 passed / 1 failedは履歴であり、今回の全体成功件数として足し合わせない。確認用dev-runtime成果物は前回のEffeTune v0.12.0を配置したものなので、今回のv0.11.1差し替え済み配布物としては扱わない。今回は文書修正と指定の再検証だけで確認用buildの再作成はしていない。製品binaryの起動・commitなし。

### 常に左→右への方向変更 (2026-10-04)

利用者の実機確認でメーター表示はOK。右綴じ・左綴じが混在する一覧を見やすくするため、常に左から右へ伸ばす決定を反映した。記録/DB/mapからrtlを除去し、追加列はordinal/totalだけ。既存2列DBの後方互換、NULL非表示、HUD読み順/見開きanchor、map更新と既存待機経路は維持する。方向・schema・描画と関連テスト/文書だけを変更し、他機能の既存差分は保持する。独立レビューはP1/P2指摘なし。commit・製品binary起動なし。

指定検証は当該未コミット差分・通常featureで実行 (`CARGO_BUILD_JOBS=1`)。ログは `target/book-resume-ltr-*.log`。

| コマンド | exit / 結果 |
| --- | --- |
| `cargo fmt` / `cargo fmt --check` | 0 / 0 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 |
| `cargo test -p mimageviewer --lib book_resume` | 0、30 passed。旧2列DBの移行後がpath/page/ordinal/totalだけであること、2列追加のtransaction rollback、anchor/比率/map/設定保存、方向変更でも記録値が不変を確認 |
| `cargo test -p mimageviewer --lib thumb_overlay_layout` | 0、20 passed |
| `cargo test --test ui_snapshot` | 0、71 passed。既存の他機能snapshotも通常比較で成功 |
| `python scripts/check_ui_glyphs.py` | 0、dangerous glyphsなし |
| `cargo test -p mimageviewer --lib remote_static_page_progress_is_the_next_pc_open_position` | 0、1 passed。2列NULL記録を確認 |
| `cargo test -p mimageviewer --lib remote_nested_page_without_resume_preserves_previous_root_meter` | 0、1 passed。既存root位置/メーター不変 |

`UPDATE_SNAPSHOTS=1` は `--lib book_resume_meter_snapshot` (exit 0、3 passed) と `--test ui_snapshot preferences_book_resume_meter` (exit 0、2 passed) にだけ指定。変更した5 PNGは目視で左→右の伸び・最後までの塗り・cut opacity・帯/badge非交差、Light/Dark/高DPIと設定の説明/折り返しを確認した。他機能だけの既存変更ファイルは作業開始時のhashと一致し、`tests/ui_snapshot.rs` も変更していない。既存差分がある `manual/grid.html` も本件ではメーター段落だけを変更し、ファイル整理先の段落は保持した。

確認用ビルドも `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 1` がexit 0。core / Remote service / EPUB PDF workerを再作成し、現在の承認済みEffeTune v0.11.1、FFmpegとVC runtimeを配置した。VC runtime PE検証も成功。ログは `target/book-resume-ltr-build-dev.log`。通常profileの利用者向け起動コマンド・注意は上記と同じ。今回はFolder/ZIP/PDFを読んで一覧へ戻り、右綴じ/左綴じとシークバー方向にかかわらずメーターが左→右へ伸びることを確認する。製品binaryはagentが起動していない。

## 10. §1.333 動画・音声の前回再生位置 (2026-10-05)

状態: 実装・自動検証完了、独立レビューP1/P2指摘なし。HEAD `c56632af7` の既存§1.256を拡張する。利用者の決定は、表示設定の共有・既定ON、長さバッジOFFでもバー表示、見終わった動画は位置なしのままバーなし、Audioを含める、すべて常に左→右。

### 10.1 既存のコード事実と変えない正本

- `src/settings.rs:5553` の `video_resume_positions: HashMap<String,f64>` は起動時から読込済み。Video/Audioの位置は `src/app.rs:88350` の保存規則により、再生直後・末尾数秒・EOFで行を除去する。出荷済みの形式・保存条件は変更しない。
- キーは `src/adjustment_db.rs:550` のnormalize (ドライブ保持・小文字・slash統一)。本のresume keyはDriveStrippedなので同じ正規化を流用できない。`src/app.rs:59635` の媒体のmetadata keyは同じドライブ保持normalize。
- 長さバッジは `src/app.rs:60572` が `DetailsLazyMeta.media.read().duration_secs` を読む。`src/app.rs:61138` は取得済みsource stampとの一致だけをメモリで検査し、UIでstat/DB/probeを行わない。既存のdetails-meta workerが親catalogのvideo_meta cacheを読んでmissだけprobeし、取消/世代/mtime-size照合を行う。
- HEAD `c56632af7` では `thumbnail_media_duration_enabled` とvisible-stage/selection-only/target要求は長さバッジだけで決まっていた。§1.333の `thumbnail_media_metadata_enabled` (`src/app.rs:59820`) はこの取得要件を共通表示設定とのORへ変更し、長さバッジの描画条件は分離して保持する。

### 10.2 状態を増やさない取得・描画

別の再生位置map、worker、監視、generation、保存列を作らない。比率の分子は毎回既存のlive `Settings.video_resume_positions` を読む。したがって再生中の位置更新・EOF/clear/delete/renameの既存表更新が、一覧へ戻った描画にもそのまま反映される。

既存 `BookResumeMeters` のPathBuf key memoを1箇所のまま本/媒体それぞれの正規化済み文字列へ拡張する。pathそのものをキーにし、idx・viewer・同名別ドライブへの取り違えを避ける。毎frameのpath正規化は不要。既存のscope除去に伴うmemo失効も維持する。

可視+近傍の長さ取得は§1.308のstaged details-meta scanに乗せる。Thumbnail表示で `thumb_show_media_duration || thumb_show_resume_meter` のとき、既存の要求・cached result・generation・scroll idle gate・取消ownerを使う。どちらもOFFなら従来の専用選択情報/AI等の要求だけに戻す。設定変更は既存のmetadata要件失効へ接続する。新しい待機状態は不要で、取得のmodal化はスクロールを妨げるため採用しない。

Video/Audioの可視cellは位置と、現在source stampに一致したメモリ上の長さを使う。位置なし・長さ未取得/不明/0/負数/失敗・source不一致、非有限値、位置<=0、位置>長さでは帯自体を描かない。比率はposition/durationでありclamp・0%/100%への代用・長さの推測をしない。等しい有効値なら1.0だが、通常の見終わりは既存保存規則が行を消すので表示しない。本は従来のordinal/totalであり、見開きanchorと対象判定を維持する。

描画helperは本・Video・Audio共通の有効fractionを受け、常に左端から塗る。現在の配置は§5.1 / §14に従い、共有設定ONで位置なし・画像・その他を含む全セルに固定下端9pt帯を予約し、下端ラベルを一律13pt上へ移す。OFFは予約なしの従来位置へ戻す。セル高・順序・画像fit・中央アイコンは維持し、タグhit-testは同じBadgePlacementを使う。色は同じos_theme helper、cut opacityも維持する。Remoteのwire/IPC版は変更しない。

### 10.3 共有設定・未リリースの改名

`thumb_show_resume_meter`、既定/欠落ON、環境設定「表示 → サムネイル」。表示名は「本・動画・音声のサムネイルに前回の位置を表示」。読み/再生位置を常に左→右へ表示し、位置や長さが不明なら非表示、見終わった媒体も非表示、長さバッジとは独立であることを説明する。旧 `thumb_show_book_resume_meter` は未出荷の§1.256で追加した項目のため移行/aliasを作らず改名する。出荷済みの再生位置表やvideo_meta形式は変更しない。

### 10.4 検証・文書更新

比率の境界/非有限/未取得/失敗/source不一致、動画/音声、位置表の即時更新・除去、異なるdrive/idx差替え、長さバッジOFFでも可視/近傍worker要求が出ること、OFF/OFF時の要件失効、既存worker世代/取消を検証する。純layoutは長さバッジあり/なし・件数・dense・極小cellの非交差。本/媒体のLight/Dark snapshot、実Preferences編集→本番OK→save→DB再読込→開き直し/Cancel/欠落ONを確認する。

spec/display-pipeline/architecture/async、設定共有のexport表と検索項目、manualのgrid/settings/読書チュートリアルを更新する。利用者向け説明に内部用語・バージョンを載せない。backlog §1.333は指定どおり「実装済み (レビュー前)」とする。fmt/core check/関連lib/UI snapshot/glyph/full gate/PreserveRuntime確認用buildの結果を後記する。commit・製品binary起動は行わない。
### 10.5 実装・検証記録 (2026-10-05)

実装箇所は `src/app/book_resume_meter.rs` (媒体のキーmemoと比率/参照)、`src/app.rs` (既存長さ取得要件のOR)、`src/ui_main.rs` / `src/app/grid_paint.rs` (共通描画)、`src/thumb_overlay_layout.rs` (媒体非交差テスト)、settings/preferences/search/export (共有設定)、関連libテストとsnapshot。`book_resume.db`、再生位置の保存規則・形式、Remote wireは変更していない。

独立レビュー (`gpt-6.1-sol` / `xhigh`) はP1/P2指摘なし。媒体を本のDB読込/物理一覧判定から独立して参照し、キーのドライブ保持、source stamp検証、既存workerの世代/取消、長さバッジ独立、共通LTR描画を確認した。

| コマンド | exit code / 件数 |
|---|---|
| `cargo fmt` / `cargo fmt --check` | 0 / 0 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 |
| `cargo test -p mimageviewer --lib resume_meter` | 0 / 41 passed |
| 同 `--lib media_duration` | 0 / 18 passed |
| 同 `--lib thumbnail_selection_info_keeps_single_target_fast_path` | 0 / 1 passed |
| 同 `--lib thumb_overlay_layout` | 0 / 22 passed |
| 同 `--lib settings_transfer` | 0 / 15 passed |
| 同 `--lib preferences::search_index` | 0 / 11 passed |
| `cargo test --test ui_snapshot` | 0 / 76 passed |
| `python scripts/check_ui_glyphs.py` | 0 / dangerous glyph 0 |
| `.\scripts\test-full.ps1 -SuppressCrashDialogs` | 0 / 11,377 passed、58 ignored |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | 0 / core・remote・EPUB worker配置済み、runtime=4 / PE=3検査通過 |

full gateはworkspaceとvendor/egui・egui-wgpu・eframeの全段通過。件数はfiltered-out=0の57集計の合計で、子process内の限定再実行2件は重複計上しない。本体libは10,282 passed / 52 ignored。

関連libは`CARGO_BUILD_JOBS=1` / `RUST_TEST_THREADS=4`、full gateにも同じprocess-local設定を使用。初回の媒体テストは新snapshot未生成4件とfixtureの対象key期待値1件で失敗した。worker起動時は取得対象の媒体keyだけを保持する既存処理 (`src/app.rs:61000`) にfixtureを合わせ、再実行は全件通過。UI snapshot初回は共有設定の文字変更2件のみ差分となり、対象を限定して更新した。意図しないsnapshot差分はない。

新規媒体Light/Dark×長さバッジON/OFFの4枚と、共有設定の2枚を目視確認した。バーは左→右、長さバッジと非交差、極小セルは既存表示優先、cut時は既存opacityを維持。PNGとテストを同時更新し、製品binaryは起動していない。snapshot更新コマンドは `UPDATE_SNAPSHOTS=1 cargo test -p mimageviewer --lib media_resume_meter_snapshot` (exit 0 / 4 passed) と `UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshot preferences_book_resume_meter` (exit 0 / 2 passed)。確認用coreは9m03s、companionは各0.3sでbuild完了。未解決の自動検証/レビュー指摘はない。ログは `target/media-resume-*.log`。

利用者の実機確認: 1分以上の動画と音声を中ほどまで再生して一覧へ戻る→現在位置のバー、長さバッジOFFでもバー、共通設定OFFで本/動画/音声のすべて非表示、見終わった媒体はバーなし、Light/Dark・小さいセルで既存バッジと重ならないことを確認する。

### 10.6 変更ファイル一覧

- 取得・描画・回帰テスト: `src/app.rs`, `src/app/book_resume_meter.rs`, `src/app/book_resume_meter_tests.rs`, `src/app/media_resume_meter_tests.rs`, `src/app/grid_paint.rs`, `src/app/tests.rs`, `src/thumb_overlay_layout.rs`, `src/ui_main.rs`。
- 共有設定: `src/settings.rs`, `src/settings_transfer.rs`, `src/ui_dialogs/preferences.rs`, `src/ui_dialogs/preferences/pages.rs`, `src/ui_dialogs/preferences/search_index.rs`。
- 設計/仕様: `docs/README.md`, `docs/architecture-overview.md`, `docs/async-architecture.md`, `docs/book-resume-meter-plan.md`, `docs/display-pipeline.md`, `docs/next-release-backlog.md`, `docs/settings-export-import-plan.md`, `docs/spec.md`。
- 利用者向け説明: `htdocs/mimageviewer/manual/grid.html`, `htdocs/mimageviewer/manual/settings.html`, `htdocs/mimageviewer/manual/tut-reading.html`。
- snapshot (6枚): `tests/snapshots/media_resume_meter_light.png`, `tests/snapshots/media_resume_meter_dark.png`, `tests/snapshots/media_resume_meter_badge_off_light.png`, `tests/snapshots/media_resume_meter_badge_off_dark.png`, `tests/snapshots/preferences_book_resume_meter_light.png`, `tests/snapshots/preferences_book_resume_meter_dark.png`。

## 11. §1.333「最後まで視聴」の記録 (2026-10-05追加決定)

状態: 実装完了・全体ゲートと確認用ビルド済み、利用者の実機確認待ち。通常コピー/移動は対象外、3秒未満はwatched保持という設計担当の決定を反映済み。前段のコミット未実施差分に追加し、製品起動はしない。

### 11.1 前提差と合意した対象範囲

`video_resume_positions` は共通 `rename_key_migration::STORES` に登録されていない。Appがrename成功/回復時にlive mapを移行し、Settingsの既存full保存でDBへ反映する。通常Shell整理のcopy/moveは `src/ui_dialogs/file_organize.rs:504` で明示的にメタデータ転記を行わず、外部変更検知へ戻す。内容identityの対象はImage/Zip/Pdf等でVideo/Audioを含まない。このため「通常copy/moveも既存resumeと同じ場所へ足すだけ」はコードと一致しない。

設計担当決定 (2026-10-05): 既存のrename/deleteに揃え、通常コピー/移動 (ファイル整理先・クリップボード貼り付け等) は対象外。整理先への移動は利用者が「エクスプローラー移動相当」と決定済みであり、コピー/移動したファイルでは再生位置と同じく最後まで見た記録も引き継がれない。watchedだけ異なる扱いにしない。この経路は変更せず、Shellのprogress sinkや新しい転記状態は追加しない。generic storeのcopy capability自体はwatchedにも登録し、pure DB APIのcopyはテストするが、実媒体のUI copy対応完了とは扱わない。

### 11.2 状態と保存規則

追加の保持状態は `Settings.video_watched_to_end: HashSet<String>` 一つだけ。キーは出荷済みresumeと同じドライブ保持normalize。`settings.db` に `video_watched_to_end(path_normalized TEXT PRIMARY KEY, updated_at INTEGER NOT NULL)` をCREATE IF NOT EXISTSで足し、既存複合値と同じtransactionでsave/loadする。settings_kvへ集合を二重保存せず、設定export/importからもpath履歴として除外する。新表は未リリース追加、出荷済みresume表・HashMapの形と意味は保持する。旧DBはinit_schemaで空表を作り、既存の消えた記録の補完はしない。旧版は新表を読まず、再開動作は変わらない。

保存の唯一の判定は既存 `save_video_resume_position` に集合を渡す。EOF→resume削除/watched追加、EOF以外で3秒未満→resume削除/watched不変、末尾5秒以内→resume削除/watched追加、その他途中→resume保存/watched解除。開始直後や再開playerの初期化で既視聴記録を消さないため3秒未満は保持する。短い動画もEOFなら記録し、EOF前は従来の<3判定順を保つ。値は既存の検証済みRemote/ローカルplayerから渡す。

通常tick/手動保存/save_allは既存apply_media_resume_updatesを、detached終了はread-only teardown planから既存apply_viewer_context_media_resume_updatesを通り、両方とも同じ保存helperへ渡す。Remoteも既存RecordVideoProgress→apply_remote_media_resume_update→共有helperを使う。wire/IPC版・再開helper・previewの復元は変更しない。watched-onlyには再開位置が無いので先頭から開く。

### 11.3 引継ぎ・消去・鮮度

renameはresumeとwatchedを一つのmedia entryとして既存の移行先優先へ揃える。新keyにいずれかがあればその状態を保持し旧keyを捨てる。新keyが空なら旧位置/既視聴を移す。DB generic watched move/copyも同transaction内で新keyのresume行を確認し、resumeが既存ならwatchを追加しない。prefixは既存exact helper経由、旧schemaで新表/resume表が無いfixtureは欠落を許容する。

通常deleteは既存path scope matcherでlive watchedも除去する。明示メタデータ整理はcommit済みreport.deleted_keysで集合を除去し、次のSettings full保存で削除行が復活することを防ぐ。既存purge retryの結果にはworkerで不在を確認したremoved_pathsを一過性payloadとして載せ、受信時に集合だけを同じscopeで除去する。新worker・再読込・pending・状態機械は作らない。このpayloadは永続/制御状態ではなく既存処理の完了データである。

Preferences OKは既存live media memory mergeへ集合を加え、draft生成後の視聴/再視聴/rename/delete結果を落とさない。明示clearの意図があればresume/trackとともに空にする。Cancelはlive値を保持する。

### 11.4 描画・簡素化

Video/Audioのwatched集合参照を最初に行い、あればSome(1.0)を共通LTR painterへ渡す。長さ取得を待たず満タンとする。本の最終anchorの保存済み比率も内容を後から数え直さないため、この扱いは本と一貫する。無ければ従来の途中比率、位置も無ければ帯なし。共通設定OFFは両方非表示。色は維持し、配置は§5.1の固定ラベル・背景へ重ねる規則に従う。

本のような別map/DB worker、content監視、watch状態enumやDB読み直しは採用しない。判定と所有者を既存保存helperとSettingsへ揃え、まれな失敗への独自retry/回復は増やさない。旧版で再視聴した後の古いwatchedが残ることは、新版で途中を再記録するまで満タンが残る制限として扱う。再開位置は旧版と同じであり、triggerは作らない。

### 11.5 検証・文書更新

保存の途中→末尾→短時間再視聴→途中、EOF/短い媒体、Video/Audio、watched満タンの長さ未取得/失敗、watched-only先頭再開、live mapの即時更新、旧DB空表追加/位置保持、DB往復/clear/KV除外、exact/slash/:: rename/copy/purge・新key合成優先、Remote本番受付/遅いsequence除外、save_all/detached exit、明示cleanup/purge retry、実Preferences OK/save/reopen/Cancel/clearをテストする。

spec/display-pipeline/architecture、設定転送の分類、manual grid/settings、backlogを更新する。共有設定のLight/Dark snapshotは文言変更に合わせ限定更新・目視確認する。前段の検証記録を新watched変更の検証と混同せず、今回のfmt/core check/lib/UI snapshot/glyph/full gate/build結果は以下へ別記する。通常コピー/移動は§11.1の決定に従い対象外。

### 11.6 追加実装の検証記録 (2026-10-05、中断時点)

通常Shellコピー/移動の範囲に関する§11.1の前提差を利用者へ照会し、その判断に依存しない保存・DB・描画・既存rename/delete経路を実装、検証した。独立レビュー (`gpt-6.1-sol` / `xhigh`) の明示cleanup後の集合復活とrename先の合成状態優先のP2は修正済み。修正後の再レビューは残存P1/P2なし。通常Shellコピー/移動を実装済みとするものではない。

| コマンド | exit code | 結果 |
|---|---:|---|
| `cargo fmt` / `cargo fmt --check` | 0 / 0 | 整形済み |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | 最新のUI文言を含む最終check |
| `cargo test -p mimageviewer --lib video_watched_to_end` | 0 | 8 passed |
| `cargo test -p mimageviewer --lib resume_meter` | 0 | 50 passed |
| `cargo test -p mimageviewer --lib rename_key_migration` | 0 | 48 passed / 1 ignored |
| `cargo test -p mimageviewer --lib metadata_cleanup` | 0 | 10 passed |
| `cargo test -p mimageviewer --lib settings_transfer` | 0 | 15 passed |
| `cargo test -p mimageviewer --lib completed_video_keeps_history` | 0 | 1 passed |
| `cargo test -p mimageviewer --lib parked_media_teardown_resume_seam` | 0 | 1 passed |
| `cargo test -p mimageviewer --lib save_all_video_resume_removes_eof` | 0 | 1 passed |
| `cargo test -p mimageviewer --lib audio_mode_resume_uses_clock` | 0 | 1 passed |
| `cargo test -p mimageviewer --lib teardown_plan_bakes_eof` | 0 | 1 passed |
| `cargo test -p mimageviewer --lib detached_teardown_plan_carries_confirmed_choice` | 0 | 1 passed |
| `cargo test --test ui_snapshot` | 0 | 最終比較 76 passed |
| `python scripts/check_ui_glyphs.py` | 0 | dangerous glyphs なし |

snapshot初回は75 passed / 1 failed (exit 101)。お気に入り設定fixtureが同じサムネイルページ全体を描くため、メーター説明文の増加でスクロールバーのつまみだけが4ピクセル変わった。限定再実行でも同じ差を確認し、本文と配置は変わらないことを目視して期待画像を更新した。メーター設定Light/Darkの説明文更新も限定更新・目視済み。更新後の全76件比較が成功した。

中断時点では、今回の追加実装に対する `test-full.ps1 -SuppressCrashDialogs` と `build-dev.ps1 -PreserveRuntime` は未実行だった。範囲確定後の成功結果は§11.7に記録する。§10の前段成功結果を今回の成功証拠に流用せず、追加watched対応の確認用dev-runtimeを改めて作成した。commit・製品バイナリ起動は行っていない。

実機確認予定: 動画と音声で途中再生→一覧の部分バー→末尾/EOF→満タン→再度開くと先頭→3秒以上の途中で閉じると部分バーへ戻る。再度開いて3秒未満で閉じた場合は満タンを維持する。以前に見終えて位置行がない媒体は帯なし。長さバッジOFF/未取得でもwatchedは満タン。共有設定OFF/ON、rename/delete、記録clearのOK/Cancelも確認する。通常コピー/移動では移動先に記録を引き継がないことを確認する (移動先に既存の記録がないファイルを使う)。

### 11.7 範囲確定後の全体検証 (2026-10-05)

設計担当は§11.1の推奨案を採用し、通常コピー/移動は対象外と決定した。3秒未満のwatched保持も採用済み。設計書・backlog・利用者向けgridマニュアルへ反映し、保存規則や通常Shell経路には追加変更をしない。

初回の `.\scripts\test-full.ps1 -SuppressCrashDialogs` はexit 101。本体libは10,296 passed / 3 failed / 52 ignored、他のworkspaceターゲットは成功した。失敗は共有STORESへsettings.dbのwatchedを登録したことに伴うテストfixtureの追従漏れ: `create_production_store_schemas` がsettings.dbの本番初期化を呼ばず、prefix index検査2件で空DBを検査していた。接続数検査1件もstore数の期待値が24のままだった。

`src/content_identity/restore.rs` のテストだけを修正し、fixtureで `SettingsDb::create_new` / `open` と初回 `save_full` を使う。BINARY leading indexとSEARCH planの検査は維持し、接続数の期待値は25 store + 1 ledger + 8 runtime = 34とする。候補1件/100件で一定という検査も維持する。製品実装・スキーマの修正は不要だった。`cargo fmt` はexit 0、`cargo test -p mimageviewer --lib content_identity::restore::tests` はexit 0 / 17 passed (失敗3件を含む)。ログは `target/watched-full.log`, `target/watched-restore-tests.log`。

2回目の全体ゲートはexit 101。本体libは10,298 passed / 1 failed / 52 ignored。上記3件は成功したが、既存 `old_two_connection_deferred_schema_open_can_fail_despite_busy_timeout` が `[Ok(()), Ok(())]` となった。このテストはschema読込だけをbarrierで揃え、DDL成功側のrollbackを保持しないため、他方のDDL前にロックが解放されると競合を再現できなかった。`src/content_identity.rs` の当該テストだけに、両接続のDDL試行が終わるまでrollbackを待つbarrierを追加する。SQLITE_BUSYの期待値・本番DB処理は変更しない。ログは `target/watched-full-final.log`。

`cargo test -p mimageviewer --features pack-build-tools --lib old_two_connection_deferred_schema_open_can_fail_despite_busy_timeout` はexit 0 / 1 passed。最終の `.\scripts\test-full.ps1 -SuppressCrashDialogs` はexit 0 / 11,394 passed / 58 ignored / 0 failed。本体libは10,299 passed / 52 ignored。workspace・vendor/egui・egui-wgpu・eframeの全段が成功した。filtered-out=0の57集計の合計で、子process内の限定再実行2件は重複計上しない。`CARGO_BUILD_JOBS=1` / `RUST_TEST_THREADS=4` をprocess-localで設定した。ログは `target/watched-schema-race-test.log`, `target/watched-full-verified.log`。

その後の `cargo fmt --check`、`cargo check -p mimageviewer --bin mimageviewer-core`、`python scripts/check_ui_glyphs.py` もすべてexit 0。core checkログは `target/watched-check-verified.log`。追加のテスト修正は `src/content_identity.rs` / `src/content_identity/restore.rs` のcfg(test)内だけで、製品の内容identity処理には変更がない。

`.\scripts\build-dev.ps1 -PreserveRuntime` はexit 0。normal feature set / dev-runtime profileでcore (11m50s)、Remote service (0.53s)、EPUB PDF worker (0.76s) をビルドし、`target/dev-runtime/`へ配置済み。runtime=4 / PE=3の検査も成功した。ログは `target/watched-build.log`。製品バイナリ起動・commitは行っていない。自動検証の未解決点はなく、実機確認は利用者が行う。

範囲確定後の変更ファイルは `docs/book-resume-meter-plan.md`, `docs/next-release-backlog.md`, `htdocs/mimageviewer/manual/grid.html` と、全体ゲートで見つかったテスト不備を直す `src/content_identity/restore.rs`, `src/content_identity.rs`。後二者はcfg(test)内だけの修正である。

## 12. ラベルを押し上げない重ね描き — 旧3pt配置の実装・検証記録 (2026-10-05)

**履歴: この節のラベル固定・重ね描き配置は§14の固定下端予約帯へ置き換えた。以下の成功記録は旧案の証跡として保持する。**

基準: `next-file-ops` / `570587339` 上の未コミット差分。利用者は§1.333の機能を実機確認済み。バーがあるセルだけラベルが上がる問題を解消するため、当時の§5.1の帯予約を撤去した。設定や保持状態を増やさず、既存のラベル配置一つに固定する。バーだけを実際の文字の下へ合わせ、背景への重なりを許す。DB・記録・再開・長さ取得の経路は変更しない。

### 12.1 実装と確認した前提

- `layout_thumbnail_overlays` (`src/thumb_overlay_layout.rs:432`) は一度だけラベルを配置し、候補バーを追加する。メーター有無による `inner` / 右下レーンの縮小や、ラベル維持のための二回目のlayoutは不要になった。左下は `:595`、右下は `:741` / `:760` の従来の基準を使う。
- `layout_cell_overlays` (`src/app/grid_paint.rs:336`) は既存のcached galleyから `mesh_bounds` を取得する (`:379`)。文字の原点はepaintと同じphysical-pixel roundingへ合わせ、バーだけを縮小する (`:342`)。
- `draw_cell` が比率を受け取り、ラベル→バー (`src/app/grid_paint.rs:914`) →選択枠/見開き枠 (`:916`) →チェック (`:933`) の順に描く。ライブ一覧とsnapshotが同じ入口を使う。切り取り時のバーには従来のcontent opacityを適用する。
- Barあり/なし/設定OFFの全ラベル・チェック矩形一致を純layoutと実フォントの双方で検査する。CJK・descenderを含む文字、通常/密集ラベル、32/48/100/140/240pt幅、整数/端数原点、100/125/150/200% DPIでglyphとの非交差を検査する。
- Light/Dark・小セル・150% DPIの本/動画/音声snapshotは対象10件だけ更新し、全10画像を目視確認した。既存7画像の更新と3画像の追加。設定のsnapshotは変更不要。
- 独立レビュー (`gpt-6.1-sol` / `xhigh`) の指摘2件: Unicode fixtureの誤変換と見開き枠の描画順を修正。最終差分の残存P1/P2はなし。

### 12.2 配置の実測

候補: 左右4pt内側、下端3pt内側、最大厚さ3pt。描画はphysical pixelへ内側丸めする。glyphに当たる候補は文字の下端から1physical pixel離して細くし、1physical pixel未満なら省略する。3physical pixels未満の細いバーには輪郭を付けない。ラベル背景は重なってよい。

実測値・今回の検証結果は以下へ記録する。前段§11の成功結果を今回の結果として流用しない。ログは `target/meter-overlay-*.log`。

`thumbnail_resume_meter_keeps_all_label_rects_and_clears_real_glyphs_at_each_dpi` の本番font-backed layoutで、原点(20,20)、幅140/240pt、高さ94ptの `v.mp4` / `a.mp3` を実測した。セル下端はy=114pt。日本語と `gyjpq` を含む本/媒体名、小セル、密集ラベルも非交差assertで確認している。

| DPI | ファイル名glyph下端 | 長さglyph下端 | バー上端〜下端 | 厚さ | 長さglyphからバーまで |
|---|---:|---:|---:|---:|---:|
| 100% | 100.000pt | 101.000pt | 108.000〜111.000pt | 3.000pt / 3px | 7.000pt |
| 125% | 100.800pt | 100.800pt | 108.000〜110.400pt | 2.400pt / 3px | 7.200pt |
| 150% | 100.000pt | 100.667pt | 108.000〜110.667pt | 2.667pt / 4px | 7.333pt |
| 200% | 100.000pt | 101.000pt | 108.000〜111.000pt | 3.000pt / 6px | 7.000pt |

これは当該fixtureの実測であり、固定値で全ラベルの文字位置を仮定するものではない。実際の各セルではそのラベルのglyph範囲に合わせてバーだけを縮める。背景との重なりを許す右下件数バッジも、同じ文字非交差検査を通る。

### 12.3 検証記録 (2026-10-06)

今回の未コミット差分に対して `CARGO_BUILD_JOBS=1` / `RUST_TEST_THREADS=4` をprocess-localで設定した。製品起動・commitは行わない。

| コマンド | exit code | 結果 |
|---|---:|---|
| `cargo fmt` | 0 | 整形済み |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | 本番描画経路の型確認 |
| `cargo test -p mimageviewer --lib thumbnail_resume_meter -- --nocapture` | 0 | 5 passed、配置実測・描画順・薄いバー・切り取りopacity |
| `cargo test -p mimageviewer --lib thumb_overlay_layout` | 0 | 22 passed |
| `cargo test -p mimageviewer --lib resume_meter` | 0 | 56 passed、更新後snapshotを通常比較 |
| `cargo test -p mimageviewer --lib media_duration` | 0 | 18 passed |
| `UPDATE_SNAPSHOTS=1 cargo test -p mimageviewer --lib resume_meter_snapshot` | 0 | 10 passed、対象画像のみ更新・目視済み |
| `cargo fmt --check` | 0 | 最終ソース整形確認 |
| `cargo test --test ui_snapshot` | 0 | 76 passed、既存integration期待画像は変更不要 |
| `python scripts/check_ui_glyphs.py` | 0 | dangerous glyphsなし |
| `.\scripts\test-full.ps1 -SuppressCrashDialogs` | 0 | 11,400 passed / 58 ignored / 0 failed。本体lib 10,305 passed / 52 ignored |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | 0 | normal feature set / dev-runtime、runtime=4 / PE=3検査成功 |

全体ゲートはworkspaceとvendor/egui・egui-wgpu・eframeの全段成功。filtered-out=0の57集計の合計で、子processの限定再実行2件は重複計上しない。`target/meter-overlay-full.log` を証拠とする。初回で成功し、他機能のコード・テストを追加修正する必要はなかった。

確認用coreは8m30s、Remote serviceは0.39s、EPUB PDF workerは0.36sでビルドし、`target/dev-runtime/`へ配置した。core生成時刻は2026-10-06 00:34:09。ログは `target/meter-overlay-build.log`。commit・製品バイナリ起動は行っていない。現在の自動検証に未解決点はなく、今回の配置の実機確認は利用者が行う。

実機確認: 本/動画/音声のバーあり・なし・満タンを横に並べ、共有設定ON/OFFでもタイトル・長さ・件数などのラベルの高さが変わらないことを確認する。小セルと150/200% DPI、Light/Dark、長さバッジON/OFF、選択/チェック/見開き相手cursor/切り取り表示で文字と枠が読めることを確認する。バーの余地が足りない小セルでは、バーが細くなるか省略されてもラベルは動かない。

## 13. 表示画像内の太いバーへ改訂

**履歴: この節の画像幅・障害物を避ける配置は§14の固定下端予約帯へ置き換えた。以下の比較・実測・成功記録は旧案の証跡であり、現行方式の検証には使用しない。**

2026-10-06: 実装・独立レビュー・自動検証・確認用ビルド完了。利用者の実機確認待ち。

### 13.1 配置判断と不変条件

§12のラベル固定を維持し、文字下の3pt帯から、表示画像内の公称9 logical ptの角丸バーへ変更する。厚さを増やすためにラベルを動かす予約帯は復活させない。画像幅・セル幅・文字へ半透明で重ねる案をsnapshotで比較し、不透明な画像幅のバーを採用した。色は既存 `os_theme::book_resume_meter_palette` を維持する。

画像のfit・回転を含む実際の表示矩形を `draw_thumb_texture` と共有する。Folderは補正後texture、ZipFile/PdfFile/Videoは描画対象textureを使い、Audio/未ロードなど画像のないセルはinnerへフォールバックする。画像やその縦横比、cell高、ラベル・checkのrect、hit-testは変更しない。DB・位置の記録・長さ取得・共有設定にも変更を加えない。 本番のfit矩形は `src/app/grid_paint.rs:44`、帯の探索は `:348`、placeholderの実glyph取得は `:413`、draw_cellでのラベル後paintは `:991`。再生/音楽アイコンの描画範囲は既存helper (`src/ui_helpers.rs:1168`, `:1198`) が返す。

配置範囲内で実glyph（未読込時の文字を含む）・再生/音楽アイコン・check・切り取りマークに当たらない最も下の水平帯を選び、衝突時はバー全体を上へ移して9ptを保つ。全幅の9pt帯がない場合だけ、最大の空き帯へ細くして収める。1physical pixelも確保できなければ省略し、ラベルは動かさない。実glyphとの1physical pixelの間隔、描画と同じ文字原点のpixel丸め、バーのpixel境界への丸めを使う。ラベル背景への重なりは許す。描画順はラベル→バー→選択/見開きcursorの枠→check、切り取り内容のopacityは従来どおりとする。

### 13.2 比較と実測

比較PNGは `tests/snapshots/thick_resume_meter_comparison_{light,dark,light_high_dpi,dark_high_dpi}.png`。各画像の左列が採用案（画像幅・不透明・文字を避ける）、中央がセル内幅、右が画像下端に半透明45%を重ねる案。Light/Dark・白/黒表紙・縦長/横長・動画/音声・小セルを比較した。

- **採用: 画像幅・不透明**。縦長の表紙と幅が揃い、画像の中に収まる。塗り/未塗り/輪郭が白黒表紙の上でも明確で、文字を覆わない。文字・アイコンに当たる場合はバーだけを上へ動かすので、画像下端に完全固定ではない。
- セル内幅: 縦長表紙の左右の余白まで横に伸び、表紙との対応が弱い。横長や音声では差が小さいが、縦長/横長が混ざる一覧には画像幅を選ぶ。
- 半透明45%: 本の名前や形式文字へ重なる例があり、Darkの黒表紙とLightの白表紙で塗り/未塗りの区別が弱くなる。不採用。ラベルより先に描く案も、ラベル背景で太い部分が隠れて旧3pt相当の見た目に戻るため採用しない。

実測は180×108 logical ptの比較fixture（左右4pt内側）。以下は採用列のpixel内側丸め後の矩形。全例でglyph交差数は0、ラベルrectはバーなし/設定OFFと同一。Folderの密集ラベルでは9pt帯のためバーだけを大きく上へ移す。

| DPI / 種類 | 表示画像の幅 | バー x範囲 | バー y範囲 | 厚さ |
|---|---:|---|---|---:|
| 100% / Folder・縦長白 | 60.000pt | 68.000〜128.000 | 86.000〜95.000 | 9.000pt / 9px |
| 100% / ZIP・縦長黒 | 60.000pt | 68.000〜128.000 | 223.000〜232.000 | 9.000pt / 9px |
| 100% / PDF・横長白 | 166.667pt | 15.000〜181.000 | 336.000〜345.000 | 9.000pt / 9px |
| 100% / Video・横長黒 | 166.667pt | 15.000〜181.000 | 451.000〜460.000 | 9.000pt / 9px |
| 100% / Audio・固定アイコン | 172.000pt | 12.000〜184.000 | 565.000〜574.000 | 9.000pt / 9px |
| 150% / Folder・縦長白 | 60.000pt | 68.000〜128.000 | 112.667〜121.333 | 8.667pt / 13px |
| 150% / Video・横長黒 | 166.667pt | 15.333〜181.333 | 472.000〜480.667 | 8.667pt / 13px |

公称9 logical ptを画像比率で変えず、内側pixel丸めで100/125/150/200%の厚さは9.000/8.800/8.667/9.000pt（9/11/13/18px）。glyphとの最小余白は1physical pixel。角丸は2 logical pt、輪郭は1physical pixel。9pt帯がない小領域は最大の空き帯へ縮め、1pixel未満なら省く（5ptだけ空くfixtureは5pt、0.5pixelのfixtureは省略）。固定ラベルの32/48/100/140/240pt・端数原点・CJK/descender・密集バッジ、実textureの縦横比/補正/全回転、未ロード/失敗時の文字と再生/音楽アイコンを本番draw_cellから検査する。輪郭は3physical pixels未満なら省略し、細いバーの塗りを残す。

既存の本/媒体の10画像を今回の9pt表示へ更新し、比較4画像を追加した。全14画像を目視確認済み。元の描画と同じshapeのboundsを返すplay/music helperとplaceholder glyph boundsで、アイコンの見た目を変えず交差を避ける。独立レビューで見つかった未読込Video等の文字交差は、この所有境界で修正し実draw_cell回帰テストを追加した。保持状態は増やさない。

### 13.3 今回の検証記録

`CARGO_BUILD_JOBS=1` / `RUST_TEST_THREADS=4`。ログは `target/meter-thick-*.log`。追加fixtureの初回compileにDebug未実装型の診断文字列が含まれていたためexit 101となり、テストの診断文字列を修正して再実行した。製品コードにDebug実装は足していない。

| コマンド | exit code | 結果 |
|---|---:|---|
| `cargo fmt` / `cargo fmt --check` | 0 | 整形済み |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | 本番経路の型確認 |
| `cargo test -p mimageviewer --lib thumbnail_resume_meter -- --nocapture` | 0 | 9 passed、実glyph/placeholder/iconと厚さ/画像幅/回転 |
| `UPDATE_SNAPSHOTS=1 cargo test -p mimageviewer --lib resume_meter_snapshot -- --nocapture` | 0 | 14 passed、対象のみ更新・全画像目視済み |
| `cargo test -p mimageviewer --lib thumb_overlay_layout` | 0 | 22 passed |
| `cargo test -p mimageviewer --lib resume_meter` | 0 | 64 passed、更新後snapshot通常比較 |
| `cargo test -p mimageviewer --lib media_duration` | 0 | 18 passed |
| `python scripts/check_ui_glyphs.py` | 0 | dangerous glyphsなし |
| `cargo test --test ui_snapshot` | 0 | 76 passed、integration期待画像は変更不要 |
| `.\scripts\test-full.ps1 -SuppressCrashDialogs` | 0 | 11,408 passed / 58 ignored / 0 failed。本体lib 10,313 passed / 52 ignored |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | 0 | normal feature set / dev-runtime、runtime=4 / PE=3検査成功 |

独立レビュー (`gpt-6.1-sol` / `xhigh`) の修正後のソース・snapshotで残存P1/P2はなし。比較PNGの再生三角はLight/Dark同一の白画素群も確認済み（100%: 73画素、150%: 180画素）。

全体ゲートは初回で成功。filtered-out=0の57集計を合算し、子processの限定再実行2件は重複計上しない。本体libは890.01s。`target/meter-thick-full.log` を証拠とする。確認用coreは8m59s、Remote serviceは0.48s、EPUB PDF workerは0.42sでビルドした。`target/dev-runtime/`へ配置済み。core生成時刻は2026-10-06 01:52:10、ログは `target/meter-thick-build.log`。§12は旧3pt配置の成功記録として保持し、今回の9pt配置へ流用しない。製品バイナリ起動・commitは行わない。


### 13.4 変更範囲と実機確認

製品側は `src/app/grid_paint.rs`、`src/thumb_overlay_layout.rs`、`src/os_theme.rs`、`src/ui_helpers.rs`、`src/ui_main.rs`（前段のラベル固定差分を含む）。文書は本計画、`docs/spec.md`、`docs/display-pipeline.md`、`docs/next-release-backlog.md`、`htdocs/mimageviewer/manual/grid.html`。snapshotは本/媒体10画像と比較4画像。その他の機能・DB/設定・保存/再開・長さworkerは変更していない。HEADは`570587339`のまま、commit・製品バイナリ起動はなし。自動検証の未解決点はない。

実機では本/動画/音声、縦長/横長、白/黒の画像を横に並べ、Light/Darkと100/150/200% DPIで塗り/未塗りの区別と文字の可読性を確認する。共有設定ON/OFF、バーなし/途中/満タン、長さバッジON/OFF、選択/チェック/切り取り、小セルでタイトル・長さなどのラベルの高さが変わらず、バーだけ移動/縮小/省略されることを確認する。確認用coreは通常の `%APPDATA%\mimageviewer` を使うため、利用者がインストール済み/トレイ常駐のmIVを閉じてから起動する。実機確認は未実施で、利用者が行う。

## 14. 全セルに固定下端帯を予約する方式へ再改訂

2026-10-06: 利用者の実機画像 `target/user-report/meter-video-overlap.png` を受け、§13の画像幅・障害物を避ける配置を撤回した。現行正本は§5.1。予約帯改訂時の自動検証・独立レビュー・確認用ビルドの結果を§14.3に保持する。後続P2の検索セル名消失修正と修正後gateの成功記録は§14.4、利用者の実機確認は未実施。§12/§13やP2修正前の成功結果を修正後gateへ流用しない。

### 14.1 観測された失敗と根因

期待する不変条件は、同じ共有設定状態なら、未読・途中・満タンと本・動画・音声・画像・その他を混在させても、全セルの帯・下端ラベルのgeometryが揃うこと。以前の予約方式では、layoutへ設定boolを渡すべきところで `fraction.is_some()` を渡し、有効な位置があるセルだけが帯を予約していた。そのため記録の有無で下端ラベルの高さが変わった。予約自体を取り除くことで回避するのではなく、設定を配置の唯一の入力に戻して全セルで同じ量を予約する。

§13の方式は、glyphと中央アイコンを避け続けるため、動画ではバーが画像上部まで移動し得る。画像幅への追従も縦長/横長でバー幅を変え、一覧で割合を比較する用途に適さなかった。固定下端帯を予約すれば、文字・中央アイコンのobstacle追跡や空き領域探索が不要になる。この簡素化を採用し、上移動・縮小・代替位置探索は撤去する。

### 14.2 変更範囲と受け入れ条件

- 共有設定ONで全セルに `cell.shrink(4)` の全幅・高さ9 logical pt・下端 `inner.max.y` の帯を予約する。位置なしのセルは空きを残し、有効な比率のあるセルだけtrack/fillを描く。
- 左右の下端ラベル、SearchContainerの件数、CollectionPlaceholderの単行captionを一律13pt上げる。SearchContainerの代表画像なし・未ロードの多行階層パスだけ上端を元位置に保ち、下端を13pt縮め、既存階層layoutで親階層から省略して末端名を優先する。代表画像ありは背景/パス全体の13pt移動を維持する。右下gapは3pt、左下gapは7pt。OFFは予約なしの従来位置へ戻す。長さの取得完了や位置の更新ではlayoutが変わらない。
- 写真のfit、中央の再生/音楽アイコン、cell高、並び順、位置保存・再開、watched、長さworker、DB・設定の形式は維持する。極小セルは既存の低優先badge省略とcell clipを使い、バーの位置・厚さを固定する。新しいstate/config/worker/DBを追加しない。
- 採用fillはLight `#26437A` / Dark `#8EB0EA`。従来の不透明灰色track/boundaryを保ち、選択stroke RGB(60, 120, 220)と緑Folder badgeから区別する。色の実測値は下記に記録する。
- 旧 `thick_resume_meter_comparison_{light,dark,light_high_dpi,dark_high_dpi}.png` と比較fixtureを廃止し、固定予約帯の `reserved_resume_meter_{light,dark,light_high_dpi,dark_high_dpi}.png` 4画像へ置き換える。旧案の比較と数値は§13の履歴に残す。

### 14.3 固定予約帯改訂時の検証記録 (後続P2修正前)

**過去検証: この節の階層caption全体を13pt移動して省略する方式と、そのソースに対する成功記録は§14.4のP2修正前の結果。現行仕様は§5.1、修正後gateは§14.4へ記録する。数値・ログ・ビルド時刻は履歴として保持し、後続修正に流用しない。**

実装担当からの実測・修正記録:

| 項目 | 値 / 判断 |
|---|---|
| raw帯 | cell左右4pt内側、高さ9 logical pt、下端はcell下端から4pt |
| 下端ラベル予約 | 13 logical pt。右下gap 3pt、左下gap 7pt |
| 100/125/150/200%の描画厚さ | 9/11/13/18 physical pixels、9/8.8/8.667/9 logical pt |
| pixel整列 | 左端ceil、右端/下端floor。上端は丸めた下端から整数pixel厚さを戻すため、元の矩形の上端に対する完全な内側丸めではない |
| 採用fill Light / Dark | `#26437A` / `#8EB0EA` |
| fill/trackのコントラスト比 Light / Dark | 7.91 / 6.26 |
| fill/選択strokeのコントラスト比 Light / Dark | 2.27 / 1.94 |

captionの13pt移動後を計測すると、高さ94ptのSearchContainerの深い階層パスで固定アイコンへ2.2pt、高さ48ptのCollectionPlaceholderのファイル名で固定アイコンへ10.5ptの交差を確認した。ON時の固定位置のink判定で、cell・帯・固定アイコンへ収まらない低優先captionを省略して解消した。代替位置探索やアイコン移動は行わない。cellへの収まりとviewport clipを分離し、実描画回帰で一部だけ見えるcaptionが残ることも確認した。SearchContainerの代表画像ありではON時にラベル背景を元の上部アイコンより先へ描き、背景が主アイコンを覆うケースを解消した。

後続の独立レビューの目視で、位置記録のあるVideoだけ再生アイコンを通常ラベルの後に描いたため、ファイル名を覆う退行を確認した。バーと固定予約後のラベルは交差しないため、通常セルをサムネイル/内容→バー→媒体アイコン/切り取りマーク→通常ラベル→枠/checkの順へ修正した。ONでバーがある場合の主マークの遅延先はバー直後・ラベル前に限定し、極小セルでバーが主マークを隠さないことと、主マークがラベルを覆わない従来の優先関係を両立する。OFF/位置なしの元の順序、SearchContainer / CollectionPlaceholderの内容内caption規則、SearchContainerのON時ラベル背景→元の上部アイコンの順序は保持した。

検証担当は親の実装担当。文書担当は文書の整合確認だけを行い、自動gateとbuildを重複実行しない。以下は当時の最終ソースの結果。旧案の成功結果は流用しない。

| 検証 | 状態 / 結果 |
|---|---|
| `cargo test -p mimageviewer --lib thumbnail_resume_meter -- --nocapture` | exit 0、9 passed。固定帯・全セル予約・ON/OFF・位置なし/途中/満タン・caption・描画順・scroll clip |
| `cargo test -p mimageviewer --lib thumb_overlay_layout` | exit 0、24 passed。32/48/100/140/240pt、端数原点、100/125/150/200% DPIの実glyph検査は上記9テストにも含む |
| `UPDATE_SNAPSHOTS=1 cargo test -p mimageviewer --lib resume_meter_snapshot` | exit 0、14 passed。本/媒体10画像とreserved 4画像のみ更新、全14画像目視済み |
| `cargo test -p mimageviewer --lib resume_meter` | exit 0、61 passed。更新後snapshotの通常比較を含む |
| `cargo test -p mimageviewer --lib media_duration` | exit 0、18 passed |
| `cargo fmt` / `cargo fmt --check` / `cargo check -p mimageviewer --bin mimageviewer-core` | すべてexit 0 |
| `cargo test --test ui_snapshot` | exit 0、76 passed。integration期待画像は変更不要 |
| `python scripts/check_ui_glyphs.py` | exit 0、dangerous glyphsなし |
| `.\scripts\test-full.ps1 -SuppressCrashDialogs` | exit 0、11,410 passed / 58 ignored / 0 failed。filtered-out=0の57集計を合算し、限定再実行1集計は重複計上しない。本体lib 10,315 passed / 52 ignored、793.80s |
| 独立completion review | `gpt-6.1-sol` / `xhigh`、修正後ソースと全14 PNGで残存P1/P2なし。ON記録あり/なしVideoの帯より上の画素差はLight/Darkとも0 |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | exit 0。normal feature set / dev-runtime、runtime=4 / PE=3検査成功。core 7m59s、Remote service 0.38s、EPUB PDF worker 0.37s |
| 利用者の実機確認 | 未実施 |

絞り込み検証は `CARGO_BUILD_JOBS=1` / `RUST_TEST_THREADS=4`。今回のログは `target/meter-reserved-*.log`。最終geometryは `meter-reserved-measure-final.log`、更新後の通常snapshot比較は `meter-reserved-resume_meter.log`。初回のfixtureの型指定と、切り取り色を未変換で比較したテストの診断を修正して再実行した。描画順・captionの修正後の最終ソースで上記のexit 0を確認した。

製品側の変更は `src/thumb_overlay_layout.rs`（設定で全セルの帯を予約、左右の下端レーン）、`src/ui_main.rs`（両入口で設定boolを渡す）、`src/app/grid_paint.rs`（固定帯・描画順・特殊caption・headless検査）、`src/os_theme.rs`（配色）、`src/ui_helpers.rs`（既存階層pathレイアウト関数のcrate内公開だけ）。文書は本計画、`docs/spec.md`、`docs/display-pipeline.md`、`docs/next-release-backlog.md`、`htdocs/mimageviewer/manual/grid.html`。本4・媒体6・固定帯比較4のPNGは `tests/snapshots/`。UI同期I/Oや保持stateを追加せず、DB・位置保存・再開・watched・長さworker・設定の形式は変更していない。

端数原点の140×140ptセル（min=20.3,20.7）の描画rectも記録する。公称内幅は132pt、pixel整列で左右だけを丸める。ラベルrectと実glyphは全DPIで非交差。

| DPI | バー x範囲 (pt) | バー y範囲 (pt) | 物理厚さ |
|---|---|---|---:|
| 100% | 25.000〜156.000 | 147.000〜156.000 | 9px |
| 125% | 24.800〜156.000 | 147.200〜156.000 | 11px |
| 150% | 24.667〜156.000 | 148.000〜156.667 | 13px |
| 200% | 24.500〜156.000 | 147.500〜156.500 | 18px |

実機では本/動画/音声/画像と未読・位置なし・途中・満タンを横に並べ、ONで全セルの下端ラベルと帯の高さが揃い、OFFで全セルの下端ラベルが一緒に従来位置へ戻ることを確認する。Light/Dark・白/黒表紙・100/150/200% DPI、長さバッジON/OFF、選択/チェック/切り取り、小セル、検索・コレクションのcaptionも確認する。今回の確認用coreは `target/dev-runtime/mimageviewer-core.exe` に配置済み（2026-10-06 09:53:37）。今回のログは `target/meter-reserved-build.log`。通常coreは実際の `%APPDATA%\mimageviewer` を使うため、インストール済み/トレイ常駐のmIVを閉じてから利用者が起動する。製品バイナリ起動・commitは行わない。

### 14.4 後続P2: 通常サイズのSearchContainerで末端名が消える問題

2026-10-06: 後続レビューで、共有設定ONの180×94ptという通常サイズのSearchContainerの代表画像なし・未ロード経路でも、名前が丸ごと消えるP2を確認した。§14.3のcaption交差検査は、階層パス領域全体を13pt上へ移した結果、固定アイコン下にあった領域の上端まで持ち上げ、実際のinkがアイコンへ交差すると階層パス全体を省略していた。交差しないことだけを検証しても、利用者がセルを識別する末端名の維持を確認できていなかった。

現行の修正は、代表画像なし・未ロードの階層パスだけ上端を固定アイコン下の元位置に保ち、下端を13pt縮める。アイコンも帯も移動せず、縮めた矩形へ既存の `layout_path_hierarchy` を再適用し、入りきらない親階層から省略して末端の本/フォルダ名を優先する。通常サイズのセルでは末端名の全消去を許さない。SearchContainerの件数とCollectionPlaceholderの単行captionの13pt移動・低優先省略、ON時のラベル背景→元の上部アイコン、OFF時の元配置/描画順、cellとviewport clipの判定分離は維持する。新しい配置探索・主アイコン移動・state・worker・DBは追加しない。

独立レビュアーは、この上端保持/下端縮小を代表画像ありにも広げると、180×94ptでパス領域が2.68ptになり、パスが既存の件数表示へ寄る副作用を確認した。P2の根因は代表画像なし・未ロードの経路に限られるため、代表画像ありの背景/パスは、もともと小さい固定種別アイコンより下にある元の全体13pt移動へ戻し、領域の高さを維持する。修正範囲を根因の経路へ限定することに独立レビュアーと合意した。

修正後の回帰では、180×94ptの通常セルと深い階層パスで、ON/OFF・代表画像あり/なし・Light/Dark・100/125/150/200% DPIに対して末端名が表示され、親階層から省略されることを検査する。固定アイコン・帯との非交差、極小セルの低優先省略、部分可視のcaption維持と既存の描画順も確認する。§14.3の9テスト/14PNG・全体gate・確認用buildの成功結果はP2修正前の履歴として保持し、今回の修正の成功証跡へ流用しない。

修正後の焦点gateは成功。検証担当は親の実装担当、条件は `CARGO_BUILD_JOBS=1` / `RUST_TEST_THREADS=4`。全GridItem種別を含む通常サイズの15ケースと長い検索末端名の計16ケースを、Light/Dark・100/125/150/200% DPI・サムネイル読込前後・OFF/ON位置なし/ON位置ありで検査する。名前を元から出さない画像/仮想ページでは評価、スタックでは枚数の存在を必須にする。全体gate・確認用buildも完了した。

180×94pt、cell原点(20,20)、100% DPIの実inkは、代表画像なしの `book` が y=57.8〜64.8pt、固定アイコンが31.0〜50.0pt、帯が101.0〜110.0pt。代表画像ありの `book` は72.2〜76.2pt、件数は77.0〜87.0ptで、固定アイコン・帯と交差しない。

| 検証 | 状態 / 結果 |
|---|---|
| `cargo test -p mimageviewer --lib thumbnail_resume_meter -- --nocapture` | exit 0、10 passed。末端名必須・全種別・非交差・部分可視・極小セル・描画順 |
| `cargo test -p mimageviewer --lib grid_paint` | exit 0、33 passed |
| `UPDATE_SNAPSHOTS=1 cargo test -p mimageviewer --lib reserved_resume_meter_snapshot` | exit 0、4 passed。reserved 4PNGのみ更新し、Light/Dark・100/150%で目視確認 |
| `cargo test -p mimageviewer --lib resume_meter` | exit 0、62 passed。更新後の通常snapshot比較を含む |
| `cargo test -p mimageviewer --lib thumb_overlay_layout` | exit 0、24 passed |
| `cargo fmt` / `cargo fmt --check` / `cargo check -p mimageviewer --bin mimageviewer-core` | すべてexit 0 |
| `cargo test --test ui_snapshot` | exit 0、76 passed |
| `python scripts/check_ui_glyphs.py` | exit 0、dangerous glyphsなし |
| `.\scripts\test-full.ps1 -SuppressCrashDialogs` | exit 0、11,411 passed / 58 ignored / 0 failed。filtered-out=0の57集計を合算し、限定子再実行2集計は重複計上しない。本体lib 10,316 passed / 52 ignored、688.40s |
| 独立completion review | `gpt-6.1-sol` / `xhigh`、修正ソース・10テストのログ・更新4PNGを確認し、P2解消、残存P1/P2なし。4PNGの検索セル内でON位置あり/なしの画素差は0 |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | exit 0。normal feature set / dev-runtime、runtime=4 / PE=3検査成功。core 57.68s、Remote service 0.35s、EPUB PDF worker 0.34s |
| 利用者の実機確認 | 未実施 |

今回の証跡は `target/meter-path-label-*.log`。coreは `target/dev-runtime/mimageviewer-core.exe` に配置済み（2026-10-06 10:35:53）。最初の検証で代表画像ありの文字サイズ変更を既存テストが検出したため、根因のないloaded経路を元の配置へ戻し、再検証した。最終ソースで上表の成功結果を確認した。HEADは `570587339` / `next-file-ops`、commit・製品バイナリ起動なし。今回追加の変更は `src/app/grid_paint.rs`、本計画・display-pipeline・specとreserved 4PNG。前段の未コミット差分は保持した。

利用者の実機確認では180×94pt程度の検索セルをLight/Darkと100/150/200% DPIで並べ、代表画像なし/読込前でも末端名が残ること、記録あり/なしで帯・アイコン・文字の位置が一致すること、設定OFFで元の配置になることを確認する。確認用coreは通常の `%APPDATA%\mimageviewer` を使い実データを更新し得るため、インストール済み/トレイ常駐のmIVを閉じてから利用者が起動する。

## 15. §1.350 変換対象書庫の一覧セル（2026-10-07、ラインA）

以下は当時の仕様・検証記録。キャッシュ削除後のメーター非表示は§18、後続RARセルの表示・開封は§19の利用者決定が優先する。サムネイル／pinの共有source失効は維持する。

利用者とmIVスレ>>529の報告では、v4.4.0のRARサムネイルにバーが出ない。
コード上の根因は`thumbnail_book_resume_meter`の対象kindから`ConvertibleArchive`を除いていたこと。
保存失敗の観測とは扱わない。初版の対象外という§2と既存テストの仕様を今回拡張する。

セルの元pathを`path_key::normalize_keep_drive`で既存`converted_archive_cache_paths`へ照合し、
`ConvertedArchiveSourceState::load_path`が返す実読込元を既存`BookResumeMeters::get`へ渡す。
Directは解決済みの元RAR/CBR（分割RARなら先頭part）、CachedZipは変換結果のZIP。
二つのDB保存keyを統合・移行せず、未登録 / Pending / Unavailable / 解決先の行無しは非表示。
元書庫の行へのfallback、セル描画中のstat・書庫検査・DB照会、保存/復元方式の変更はない。

前提のコード照合: `resolve_converted_archive_candidate_with`は分割先頭partを解決して
cache DBのstamp/実体照合をworkerで行い、直読みなら解決済みRAR pathを返す。
`poll_converted_archive_cache_paths`は既存世代/cancel検査でmapを採用し、変更時にrepaintする。
`initialize_converted_archive_cache_paths`は一覧再読込時にmapを初期化する。
この既存鮮度契約を使い、本件専用worker・監視・pending・第二のalias mapを作らない。
メーターの配置・共有ON/OFF・合成root/詳細/Remote非対象も維持する。

簡素化: 解決済みread sourceと既存全行mapを接続するだけにし、
保存key移行や元書庫/変換結果の二重記録による状態の組み合わせを増やさない。
既存barの純粋描画とsnapshotをそのまま使うので、期待PNGの更新は不要。

回帰は既存tile-kindテストのConvertibleArchive非表示期待を解決済みDirectの表示へ更新し、
元pathとcache pathへ異なる値を記録した6拡張子、未解決/失効状態、一覧再初期化、設定OFF、
解決先の行無し、分割RARのDirect/CachedZipを追加した。fake pathへのmap参照で検査し、
実RAR展開/外部変換の実行結果とは区別する。
修正前コードで関連41件は38成功・3失敗、実exit101（`target/A-1350-red.log`）。
初回の依存build失敗と追加testのborrow errorはvalid redに含めない。
修正後の結果は下表。検証担当はラインAの実装担当、HEADは
`d29bcfbec9e1bb0213ae1c4de37e5fa149fec18e` / `next-nav`に本節の未コミット差分を加えた状態。
依存buildは`CARGO_BUILD_JOBS=1`、testは`RUST_TEST_THREADS=4`。
独立レビュー・実機確認はcoordinatorへ引き継ぎ、本節の結果で代替しない。

| 検証 | 結果 / 証跡 |
| --- | --- |
| `cargo test -p mimageviewer --lib book_resume_meter_` | exit 0、41 passed / 0 failed、4.14s。`target/A-1350-green.log` |
| `cargo test -p mimageviewer --lib`（pipeなし） | exit 0、10,938 passed / 52 ignored / 0 failed、1046.83s。`target/A-1350-full-lib.log` |
| `cargo fmt` / `cargo fmt --check` | exit 0 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0、15m20s。`target/A-1350-check-normal.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core --features portable` | exit 0、1m02s。`target/A-1350-check-portable.log` |
| `python scripts/check_ui_glyphs.py` | exit 0、dangerous glyphsなし |
| `.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | exit 0、normal feature set。core 33m24s / Remote 3m07s / EPUB worker 2m15s、runtime=4 / PE=3検査成功。`target/A-1350-build-dev.log` |
| 独立レビュー / 利用者の実機確認 | 未実施 |

確認用coreは`target/dev-runtime/mimageviewer-core.exe`（2026-10-07 22:48:22）へ配置済み。
通常の`%APPDATA%\mimageviewer`を使い、実設定/データを更新し得る。
共有mutexのためインストール済み/トレイ常駐のmIVを閉じ、利用者が
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`で起動する。

利用者の実機確認候補: 直読みRAR/CBRと変換済みRAR/CBR/7z/CB7/LZH/LHAを途中まで読み、
親一覧のバーが記録位置を示すこと、分割RARの後続partが同じ本の位置を示すこと、
未変換/変換結果失効でバーを作らないこと、一覧再読込・設定ON/OFF、既存ZIP/PDF/フォルダの対照。
実アプリはこの実装担当が起動しない。

## 16. §1.350 独立レビューr3 — cache削除完了の失効（2026-10-08）

以下は当時の仕様・検証記録。キャッシュ削除後のメーター非表示は§18、後続RARセルの表示・開封は§19の利用者決定が優先する。サムネイル／pinの共有source失効は維持する。

利用者提示の独立レビューP2をコードで照合し、採用した。反対意見はない。
`poll_archive_cache_maint_pending`はDeletedSelected / DeletedMissing / DeletedAllで管理画面の
行を再読込するだけだった。一方、read source workerはPendingだけを再投入するため、
削除済みZIPをCachedZipとして保持していた。描画でexists/DBを追加する修正は採らない。

完了pollから既存read source ownerの`invalidate_converted_archive_cached_sources`へ接続する。
削除前のbatchはcancel/receiver破棄し、CachedZipだけをPendingへ戻す。
既存rangeとprefetch admissionで再判定し、cacheが消えていればUnavailableへ移る。
削除結果は件数のみなので全CachedZipを再検証する。新しい削除key記憶やalias/pending fieldを
増やすよりこの既存ownerの鮮度境界を使う。Direct / Unavailable / pin rootの来歴、
BookResumeMeters、DB保存・復元key、既に表示したtextureは変更しない。Rows/Errorは失効しない。

追加回帰は実ArchiveCacheDbへcacheを記録し、実spawn_archive削除worker→完了pollを通す。
選択 / 元ファイル消失 / 全削除の全経路でバーが消え、非同期再判定でUnavailableになること、
削除前のqueued replyを採用しないこと、Directと保存recordが残ることを検査する。
管理画面Rows/Errorは失効させない対照も追加した。
修正前の実選択削除経路は1件失敗・exit101（`target/A-r3-red.log`）。
修正後gateは下表。HEAD `c0ce50272fcfea5e9aa406ef59899479bd45756f`へ今回の未コミット差分を
加えた状態で実装担当が実行し、CARGO_BUILD_JOBS=1 / RUST_TEST_THREADS=4を使用した。
独立再レビュー・製品の実機確認は未実施。

| 検証 | 結果 / 証跡 |
| --- | --- |
| `cargo test -p mimageviewer --lib book_resume_meter_` | exit 0、43 passed / 0 failed、6.03s。`target/A-r3-meter.log` |
| `cargo test -p mimageviewer --lib archive_pin_root` | exit 0、8 passed / 0 failed、1.30s。`target/A-r3-pin.log` |
| `cargo test -p mimageviewer --lib incremental_archive_result` | exit 0、2 passed / 0 failed、0.43s。`target/A-r3-batch.log` |
| `cargo fmt` / `cargo fmt --check` | exit 0 |
| 通常core check / portable core check | 両方exit 0、18.79s / 19.30s。`target/A-r3-check-normal.log` / `target/A-r3-check-portable.log` |

利用者指示に従い、今回はfocused test・fmt・通常/portable core checkだけを実行する。
全lib・glyph・build-devは再実行せず、製品バイナリを起動しない。
§15の2026-10-07確認用バイナリには、このr3失効修正は含まれない。

## 17. r4 — 退避した解決元ownerへのキャッシュ失効（2026-10-08）

以下は当時の仕様・検証記録。キャッシュ削除後のメーター非表示は§18、後続RARセルの表示・開封は§19の利用者決定が優先する。サムネイル／pinの共有source失効は維持する。

指摘の根因は一致した。r3はmounted mapのみ失効させ、SmartFolderPreparedGridの親mapや
parked ViewerContextBundleを残したため、親→子→cache削除→BSで古いCachedZipが戻る。
ただし「Smart合成rootに古いバーが復活する」という表現には反対する。
thumbnail_book_resume_meterはSmartFolderPosition::Rootを対象外としており、この仕様は維持する。
退避mapはサムネイルの解決元としても使うため、失効漏れ自体は実在し、修正対象である。

削除完了から一つのsource-owner失効helperへ渡し、mountedと全AtRest/Retiring bundleの
map/解決batch/Smart session親payloadを同型で失効させる。Visible親のmapとOffscreen親の
prepared aggregateのCachedZipだけをPendingにする。contextをmountしない。
Direct/Unavailable、pin来歴、BookResumeMeters、保存/復元key、items/画像/viewportを維持する。
共有sort用ReusedSmartFolderMetadataや進行中prepare結果のmapも、prepared aggregateの
採用境界でCachedZipをPendingへ戻して既存の非同期解決workerへ接続する。
prepare結果を終端解決の正本としないため、cache削除より前に作られた結果を後から採用しても
削除済みZIPを再公開しない。paintのI/O、専用worker/pending/epochは追加しない。

| 退避・復元経路の照合 | 対応 |
| --- | --- |
| Smart親→子→BS、履歴←/→のresident親復帰 | Smart sessionのVisible/Offscreen payloadへ削除時に失効。共通restoreでそのmapを戻す |
| Smartのsort-only再prepare・no-resident履歴準備、進行中prepare・評価条件による行追加 | aggregate採用/merge時にCachedZip再判定。共有metadataをclone/resetせず再利用 |
| 通常folder/Rating/Collectionの履歴←/→、A/Bの地点復帰 | FolderNavHistoryTarget/QuickFolderWorkspaceは地点と履歴だけでmapを保持しない。既存load/initializeでPendingから再解決 |
| detachedのpark/mount/fork/drop | source mapとbatchをbundleが所有。AtRest/Retiringにも変異時だけ失効を渡す。forkの空初期化、swap/dropは変更しない |
| Rows/Error・read-only親復帰/履歴/context切替 | 共有ストアへの変異通知を出さず、Directや別contextの読書表示をresetしない |

簡素化: 退避root全体のreloadやmetadata再読込、contextごとの別epochを検討した。
前者は大規模Smartの移動済みgrid・選択/scroll再利用を失い、後者は新しい状態組み合わせが増える。
既存payload所有者への同じ失効と、既存aggregate採用/非同期解決へ集約する案を採用した。
初回/再prepareのCachedZipも既存range workerで確認する。UIでDB/ファイルを調べない。
detached述語/viewport/窓の切替処理は変更しない。共有ストア変異のbundle所有境界だけを扱う。

回帰は実Smart scan/prepare→実child採用→実cache削除worker/完了poll→BSのparent handler→
移動した親grid復元→実解決workerの経路で、CachedZip復活を検査する（初期化の直接呼出しなし）。
合成rootのバーは非表示のまま、sourceがPending→Unavailableになることを検査する。
さらに実sort-only prepare/adoptionで削除前の共有metadataを再利用してもCachedZipが復活しないことを検査する。
もう一件は実削除完了からparked source batchのcancel/旧reply破棄、Direct維持、
mounted context/generationとparked pan不変を確認する。
有効red: 修正前の実往復でCachedZipが復元され、Pending期待と不一致。0 passed / 1 failed、
実exit 101、0.32s（target/A-r4-red.log）。初回fixtureのrootバー表示期待で止まった実行は
仕様照合の誤りであり、有効redには数えない。

最終差分の検証（HEAD d098693403fdacf85b8c68093b5876ab40a1645c＋未コミットr4差分、
CARGO_BUILD_JOBS=1 / RUST_TEST_THREADS=4）。各filterは重複を含むため合算件数とは扱わない。
独立再レビューと実機確認は未実施。

| 検証 | 結果 / 証跡 |
| --- | --- |
| `cargo test -p mimageviewer --lib book_resume_meter_` | exit 0、45 passed / 0 failed、5.30s。target/A-r4-meter.log |
| `cargo test -p mimageviewer --lib smart_folder_transition_tests` | exit 0、103 passed / 0 failed、36.49s。target/A-r4-smart.log |
| `cargo test -p mimageviewer --lib rating_smart_` | exit 0、10 passed / 0 failed、2.19s。target/A-r4-rating-smart.log |
| `cargo test -p mimageviewer --lib archive_pin_root` | exit 0、8 passed / 0 failed、1.69s。target/A-r4-pin.log |
| `cargo test -p mimageviewer --lib incremental_archive_result` | exit 0、2 passed / 0 failed、0.34s。target/A-r4-batch.log |
| `cargo fmt` / `cargo fmt --check` | exit 0 |
| 通常core check / portable core check | 両方exit 0、14.39s / 14.06s。target/A-r4-check-normal.log / target/A-r4-check-portable.log |

利用者の指定範囲に従い、全lib/full gate・glyph・確認用buildは再実行しない。
製品バイナリを起動せず、Git commitは作らない。英語messageはtarget/A-r4-msg.txt。
§15の確認用バイナリにはr3/r4の失効修正が含まれない。
coordinatorはsource-ownerの退避/採用境界とPageIdentity契約を独立再レビューへ渡し、
[残る利用者質問](file-type-visibility-plan.md#82-残る利用者質問未回答具体例と推奨)への判断を集める。

## 18. キャッシュ削除後のバー保持（利用者決定 2026-10-08）

以下は2026-10-08時点の仕様と検証記録。後続RARセルの表示・開封は2026-10-09の§19が優先する。

実機確認を受け、solid RAR / 7z / LZH等の変換書庫は、変換キャッシュを削除しても保存済みの読書位置バーを表示する。同じdata-dirでは次回openで再変換し、同じkeyから再開する。Direct RARは従来どおり元書庫のkeyを使う。表示設定OFFでも記録を維持する。

### 保存データ・source owner

`record_book_resume`は変換後の`current_folder`（キャッシュZIP）へ保存する。`cache_zip_path_for_data_dir`は論理sourceの正規化pathからhashとbasenameを計算するだけで、mtime / sizeを含めない。同じdata-dir・sourceでは削除後の再変換もsource変更後も同じkeyになる。元書庫とZIPのkeyを移行・統合しない。

調査では、管理画面の単体・元ファイル消失・全削除、stamp不一致の掃除、容量上限のLRU整理は`converted_archives`とZIPだけを削除し、`book_resume`行を削除しない。metadata孤立掃除もDriveStrippedのbook_resumeを対象外としている。キャッシュ削除を読書位置クリアへ接続しない。利用者による明示的な位置クリア／実ファイル削除の既存規則は変更しない。

既存の`ConvertedArchiveSourceState`だけを拡張する。CachedZipは論理sourceと実読込pathを保持し、Unavailableはworkerが確認した論理sourceを保持する。metadata欠落・worker起動失敗・RARのheader確認失敗では論理source未確定（None）を明示し、推測しない。RARのheader probeで先頭volumeが確定した後、詳細inspectionが失敗してもその確定結果は残す。Smartのprepared sourceにも同じ来歴を運ぶ。

### 表示と共有失効

ConvertibleArchiveのバーはDirectならそのpath、CachedZipなら保持された実読込path（open／保存／復元と同じkey）、source確定済みUnavailableだけは現在のdata-dirと論理sourceから計算した変換ZIP pathで既存BookResumeMetersを引く。キャッシュ存在確認・stat・書庫検査・DB照会はセル描画へ追加しない。行無しやPendingは非表示。削除後の再判定中に一時的にバーが消えることは利用者が許容した。

状態削減として、別のmeter専用source mapやalias、cache削除履歴は作らない。キャッシュが存在しなければ表示しないという条件だけを外し、有効なCachedZipの実読込pathは維持する。管理画面完了poll→既存source owner失効、Smart親一覧のstash／再利用prepared payload、parked contextへの伝播、旧worker replyの取消はすべて維持する。これらはサムネイル・pinが削除済みZIPを使わないためにも必要で、メーター専用の失効経路は存在しなかった。Directやtexture、BookResumeMeters自体の失効は追加しない。

### data-dir移動時の有効cache（調整判断 2026-10-08）

profileをコピーして旧profileを残すと、cache DBのpeek／lookupは旧cacheの有効な絶対pathを返す。CachedZipではそのpathが採用・保存・復元keyであり、メーターも同じpathを使う。現在のdata-dirから再計算して別keyへ置き換えない。

移動後に旧cacheを削除すると、非同期再判定でUnavailableへ移り、現在の新data-dirから計算したpathを使う。旧keyの読書位置行は削除しないが新keyには対応付けないため、このまれなケースではバーが消え、再変換時も旧keyの位置は復元されない。状態削減としてalias／key移行／削除履歴は追加しない。同じdata-dir内の削除・再変換は従来の保持契約を維持する。

回帰は旧App／DBを閉じてarchive_cache.db・book_resume.dbを新profileへコピーし、旧ZIPを保持する。実peek／lookup・source解決・ZIP採用・位置保存／復元が旧pathを共用することと、旧cache削除後も旧保存行は残るが新keyにバーがないことを確認する。

### 回帰確認

実削除worker→完了poll→非同期source再判定で、単体／消失／全削除後も保存行とバーが残ることを確認する。header-confirmedの後続RARから先頭volumeのkeyへ解決するケース、Directとのkey分離、Pending／未登録／未知source／設定OFFの非表示も対象。実7z変換→位置記録→削除→再変換→ZIP採用／位置復元、およびsource stamp変更でcacheが失効しても同じkey／保存位置が残ることを確認する。共有sourceのSmart parent→child→削除→BS、parked context、pinの回帰も維持する。

### 実装担当の検証（2026-10-08）

| 検証 | 結果 / 証跡 |
| --- | --- |
| 修正前の実cache削除回帰 | exit 101、1 failed。削除完了・worker再判定後にNoneとなり、期待した3/5が消える。`target/A-1350-keep-red.log` |
| `cargo fmt` / `cargo fmt --check` | exit 0 / 0。変更ファイルは既存どおりUTF-8・CRLF、`git diff --numstat` / `--check`も確認 |
| `--lib book_resume_meter` | exit 0、47 passed。`target/A-1350-keep-focused-green.log` |
| `--lib archive_decision_scope` / `archive_rollup_edit_filter` / `archive_pin_root` | 各exit 0、4 / 1 / 8 passed。`target/A-1350-keep-<filter>.log` |
| `--lib convertible_archive` / `converted_rar` / `stale_incremental_archive_result_is_rejected_per_message` | 各exit 0、32 / 1 / 1 passed。`target/A-1350-keep-convertible_archive.log` / `converted_rar.log` / `stale.log`（後2件も同じA-1350-keep-接頭辞） |
| `cargo test -p mimageviewer --lib` | exit 0、10998 passed / 52 ignored / 0 failed、1255.97s。`target/A-1350-keep-lib-final.log` |
| 通常 / portable core check | 各exit 0。`target/A-1350-keep-check.log` / `target/A-1350-keep-portable.log` |
| `python scripts/check_ui_glyphs.py` | exit 0、dangerous glyph 0 |

対象filterは重複なし計94件。初回full libは今回更新した旧reply fixtureが空の現itemsを参照する誤りを検出し、途中中断した。replyが所有するsourceを渡すようテストを訂正し、その対象1件と上記full libを完走した。初回の未完了runは成功証跡に数えない。

分割RAR回帰は実volume fixtureの局所コピーでRAR5 main headerのsolid宣言とCRCを変更し、native DLLでSolid判定・先頭volume解決・実変換を検証した。外部packerやfilename推測は使っていない。7z回帰は実writer・converter・ZIP採用・resume取得を通る。描画layoutは変更せず、既存lib内のメーターsnapshotも上記filterとfull libで成功した。

利用者の指示どおりbuild-dev・製品起動・commitは行っていない。現在利用者が試しているbinaryは再作成していない。ClaudeCodeによる再ビルド後、solid RAR / 7z / LZHを途中まで読み、変換cacheを削除→一覧でバー保持（再判定中だけ一時非表示）→再openで再変換・前回位置へ復帰、後続RAR volume、設定OFFを実機確認する。コミットメッセージは`target/A-1350-keep-msg.txt`。

### data-dir移動回帰の修正・検証（2026-10-08、46e120c6b後）

CachedZipのメーターkeyを実読込pathへ揃えた。追加のコピー回帰は修正前に2/3のバーがNoneとなって失敗（exit 101、0 passed / 1 failed）し、修正後に成功した。旧Appを閉じた後の実DBコピー、peek／lookup、非同期source解決、ZIP採用、位置保存／復元、実削除完了経路を通す。初回実装の削除・再変換・source変更・後続RAR volume等の回帰も維持した。

| 検証 | 結果 / 証跡 |
| --- | --- |
| 有効red | exit 101、1 failed。`target/A-1350-keep-fix-red.log` |
| `cargo fmt` / `cargo fmt --check` | 各exit 0。変更ファイルのUTF-8・CRLF維持、numstat／diff check確認 |
| `cargo test -p mimageviewer --lib book_resume_meter` | exit 0、48 passed / 0 failed。`target/A-1350-keep-fix-focused.log` |
| `cargo test -p mimageviewer --lib` | exit 0、10999 passed / 52 ignored / 0 failed、1222.74s。`target/A-1350-keep-fix-lib.log` |
| 通常 / portable core check | 各exit 0。`target/A-1350-keep-fix-check.log` / `target/A-1350-keep-fix-portable.log` |
| `python scripts/check_ui_glyphs.py` | exit 0、dangerous glyph 0 |


今回は利用者の新しい指示に従い`.\scripts\build-dev.ps1 -PreserveRuntime`を実行し、exit 0でcore／Remote／EPUB workerを作成した（通常feature、core 2m46s、PE検査runtime=4 / pe=3成功）。証跡は`target/A-1350-keep-fix-build-dev.log`。上記初回実装時のビルド禁止記録は当時の指示として残す。製品起動・コミットは行っていない。英語コミットメッセージは`target/A-1350-keep-fix-msg.txt`。

実機確認は旧profileを残したコピー先を`--data-dir`で開き、変換書庫のバー表示と前回位置への復帰、位置更新後も同じバーを確認する。旧cacheを後で削除するまれなケースの制約は上記の仕様どおりで、保存行の破棄やkey対応付けは追加していない。

## 19. §1.355 分割RARは最初のファイルから開く（2026-10-09、利用者決定）

### 原因と決定

§1.350の実機確認で、後続巻が有効な先頭巻cacheを見落として再変換し、readerが使用中の
ZIPへのpublishがアクセス拒否になる公開済み不具合を確認した。879802c44は後続巻を先頭巻へ
解決してcacheを再利用したが、ヘッダー暗号化RARではvolume情報をpassword前に読めない。
実RAR5 fixtureで後続巻のpassword後scanが画像0件、直接変換がCRCエラーになることも確認した。

**2026-10-09利用者決定は以前のidentity／互換参照案を置き換える。後続巻の開封はサポートしない。**
workerの入口で、secretなしのnative volume headerがクリック巻をSubsequentと示した場合、
画像scan・Direct採用・cache照会・変換に進まず、以下の通知で終了する。

> 分割RARの2つ目以降のファイルです。最初のファイル（分割RAR本.part1.rar）を開いてください。

括弧内はheader確認後の既存resolutionが返した最初のファイル名を表示する。
名前のsuffixだけで後続巻と判断しない。既存の単巻`*.part2.rar`等もheaderが単巻なら開ける。
ヘッダー暗号化等でvolume番号を確認できない場合は、ファイル名から先頭巻を推測しない。
既存password flowを通し、password後のscanで画像も変換対象の入れ子も無ければ、次を通知・logする。

> 画像が見つかりません。分割RARの場合は最初のファイルを開いてください。

画像が直下に無くても入れ子の展開対象がある本は、既存の変換確認を維持する。
暗号化part1のpassword入力・変換、part1／単巻のDirect → 有効cache → 変換は維持する。
CachedZipはDBが保持する実ZIP pathを使い、data-dir移動前から残る有効cacheを読み替えない。
header暗号化で番号未確認のまま既存cacheがhitした場合は、従来どおりpassword scanを経ずに採用する。
その場合は後続巻かを判別できない。今回のheader拒否・password後0画像hintは各判定が得られた場合に適用し、
cache hitへ新たなpassword検証やfilename判定は追加しない。

### 所有境界・終了と状態削減

RAR scan workerの単一決定点で拒否し、既存typed scan outcomeのRejectedをUIの既存Error phaseへ
渡す。新しいpending／rollback／retry／移行状態は作らない。モーダルscan／password／変換、
取消token、owner証明、古いreply破棄、Errorの閉じる処理・historyの成功時採用境界を維持する。
拒否前後のcancel確認も既存worker境界に置く。UIスレッドのheader／DB照会は追加しない。
通常physical historyのpreflightはRARをtyped RarOpenとして渡し、先行content scanを行わない。
Collectionの順次候補選択もheaderで後続巻と分かればRarOpenで共通workerへ渡す。
先頭巻・単巻の空候補を飛ばす既存Collection policyは維持し、拒否の決定・通知は共通workerが所有する。

879802c44の開封用「後続巻→先頭巻へsourceを変更してDirect／cache／変換」経路と、password後の
開封source再解決を撤去する。公開済み後続巻cacheの互換peek・DB探索・aliasは追加しない。
passwordは既存scan／展開に渡す。converter内部の読込・明示batch変換は今回の開封決定とは別で、
そのreaderを改修して暗号化後続巻を読めるようにする範囲へは広げない。

### 保存データ・サムネイル・読書位置バー

v4.4.0が後続巻cache keyへ記録した位置・ページ編集は、後続巻の開封拒否により参照できなくなる
場合がある。利用者はこのまれな制約を受容した。既存ZIP・DB行・位置・編集は削除せず、移行しない。

一覧サムネイルの後続巻→先頭巻解決と共有source mapは公開済み動作として維持する。
thumbnail／pinのload、cache削除時の失効、Smart stash／parked contextの通知を変更しない。
メーターだけ、既存非同期source mapが先頭巻へ解決した後続巻セルを非表示にする。
Direct source／CachedZip.logical_source／Unavailable.logical_sourceとセルのRAR pathを純粋比較し、
一致しなければ非表示。新しいeligibility bool、描画中のI/O、ファイル名推測は足さない。
part1／単巻はDirect／実CachedZip／cacheなしの決定的keyを使い、削除後のバー・再変換時の復元を維持する。
Pending・source未確定・設定OFFは従来どおり非表示。

### 入口と維持する例外

| 入口 | 接続／契約 | 回帰対象 |
| --- | --- | --- |
| 通常open／アドレス指定 | classified open → request_rar_open_owned → 共通scan | 実handlerで後続巻を拒否、先頭巻cache再利用 |
| 履歴の戻る／進む・BS | typed history → staged archive conversion → 共通scan | 旧後続巻targetを拒否し、成功採用しない |
| Smartの子open | smart archive conversion → 共通scan | 行・帰路を保ったまま拒否、先頭巻採用を維持 |
| Rating／Collection | owner付きMainGridArchive → 共通scan | 各実handlerで拒否、先頭巻のcache採用 |
| 通常ブックマーク／閲覧履歴／起動復元 | owner付きclassified open → 共通scan | 旧後続巻entryも同じ通知 |
| 別ウィンドウの通常RAR open | DetachedGridArchive owner → 共通scan | 拒否時にcontextを採用しない |
| scanのpassword再入力 | apply_archive_password → 同じscan purpose | 暗号化後続巻の0画像hint、暗号化part1の変換成功 |
| 共通閲覧変換要求／通常Ctrl+↑↓ | owner付きOpen scan purpose | 同じ決定を共有。DFSの後続巻skipも維持 |
| 別ウィンドウの本ブックマークcache hit（P3例外） | startup_ops.rs: bookmark_detached_descriptorがcacheを直接採用 | 共通scanを通さない既存契約。missは通常openへ戻る |
| ★固定範囲の移動（P3例外） | snapshot_ops.rs: cache-only照会 | hitだけ採用、missで変換dialogを出さない既存契約 |
| 別ウィンドウDFS（既存例外） | 既存Direct／cache-only policy | header確認による後続巻skipを維持 |
| 明示sibling ZIP作成 | 同じRAR scan worker | headerで後続巻と分かればscan／変換前に拒否 |
| 明示batch ZIP作成 | converterへ直接 | 閲覧openではなく、今回のscan入口変更対象外 |

### 保存失敗の診断は維持

§1.355で追加した既存loggerへの操作・src／tmp／dst・元OSエラー・native codeの記録を維持する。
Windows wrapperは捕捉済みHRESULTからWin32 codeを保持し、後からGetLastErrorを再読しない。
通知「変換したZIPを保存できませんでした。保存先が使用中か、読み取り専用か、書き込みが許可されていません。」、
既存保存先の保持、中間ZIP掃除、no-clobberの専用通知も維持する。reader待ち・publish再試行を追加しない。

### 検証

実分割RARで通常／履歴／Smart／Rating／Collection／ブックマーク／起動／別ウィンドウの入口、
password retry、後続巻拒否前にDirect・cache・convertを採用しないことを確認する。
実RAR5暗号化fixtureで0画像hintとpart1変換を検証する。実一覧workerのDirect／CachedZip／
cache削除後Unavailableで後続巻のバーを隠し、thumbnail読込元・保存行を維持する。
part1／単巻のDirect優先、実cache path、cache削除・再変換・位置復元・設定OFFを再検証する。
新しい通知は100%／200%のsnapshotで折返しと閉じる操作を確認する。
以前のidentity検討の準備assert失敗や旧仕様のgateは、新仕様の成功証跡に流用しない。

### 実装と検証（2026-10-09、ラインA）

後続巻拒否の実Normal handler回帰は修正前にfirst cacheを採用して失敗（exit 101、0 passed / 1 failed、
`target/A-1355-spec-red.log`）。新仕様へ修正後、同じhandlerを含むRAR回帰25件が成功した。
旧仕様の後続巻バー期待が残った初回対象実行（24 passed / 1 failed）は、先頭巻の削除・再変換・復元を
検証する形へ訂正して再実行した。fixture準備の失敗は有効redに数えない。

| 検証 | 結果 / 証跡 |
| --- | --- |
| RAR handler／native worker／password／保存行 | exit 0、25 passed。`target/A-1355-spec-focused-rar.log` |
| 読書位置meter・source owner・削除／再変換 | exit 0、49 passed。`target/A-1355-spec-focused-meter.log` |
| dialog採用・hydration | exit 0、8 passed。`target/A-1355-spec-focused-dialog.log` |
| preflight | exit 0、34 passed。`target/A-1355-spec-focused-preflight.log` |
| 全lib（pipeなし、実exit確認） | exit 0、11,026 passed / 0 failed / 52 ignored、1185.75s。`target/A-1355-spec-full-lib.log` |
| UI snapshot全体 | exit 0、103 passed。`target/A-1355-spec-ui-snapshot.log`。新通知4PNGを目視確認 |
| 通常／portable core check | 両方exit 0。`target/A-1355-spec-check-normal.log`／`target/A-1355-spec-check-portable.log` |
| cargo fmt／fmt --check、glyph lint、diff --check | すべてexit 0、UI危険glyphなし。`target/A-1355-spec-fmt.log`／`target/A-1355-spec-glyph.log` |
| build-dev -PreserveRuntime | exit 0、core 9m31s、Remote／EPUB worker配置、VCRT PE check runtime=4 / pe=3成功。`target/A-1355-spec-build-dev.log` |

後続巻の8入口に加え、Direct可能な後続巻、明示sibling、実暗号化password retryを検証した。
単巻をpart2風の名前に変えても開けること、後続巻の旧cache・読書位置・ページ回転が残ることも確認した。
暗号化fixtureは公開passwordと人工PNGのみを含み、testsは一時ディレクトリへコピーする。
新fixtureの`testdata/archives/rar-header-encrypted-multipart-legacy-cache`は既存ignore対象のため、
coordinatorがcommitする際は2巻とREADMEを明示的にforce-addする。実行にWinRARは不要。

確認用binaryは`target/dev-runtime/mimageviewer-core.exe`。製品起動・commitは行っていない。
英語messageは`target/A-1355-spec-msg.txt`。独立再レビューと利用者の実機確認は未実施。
利用者はインストール済み／トレイ常駐mIVを閉じ、通常profileの確認用coreで、後続巻の最初のファイル案内、
暗号化巻のpassword後0画像hint、先頭巻のcache再利用・削除後バー保持・再変換／復元を確認する。
