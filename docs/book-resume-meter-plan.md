# §1.256 一覧の本サムネイルの読書位置メーター — 記録値を表示する設計

作成・改訂: 2026-10-04。コード調査基準: `next-file-ops` / `e804db069`。
状態: **設計のみ。独立レビュー前。製品コード変更・製品テスト・commit・アプリ起動は未実施。**
要件: [next-release-backlog.md §1.256](next-release-backlog.md#1256-一覧の本サムネイルに前回読んだ位置のメーターを表示する--438-2026-09-19)。本書の仕様判断は、2026-10-04の利用者合意によって以前の厳密な内容照合案を置き換える。file:line は調査基準時点のコード事実、追加する型・列・APIは提案である。

## 0. 合意した設計と前提

**記録時の「何ページ目 / 全何ページ / 右綴じ」を保存し、一覧はその値を表示する。** 読み順や中身を後から数え直さない。起動時の全行読込と稀なDB変更後の再読込を既存writerで行い、通常のスクロール・描画はメモリmap参照だけにする。

利用者了承済みの割り切り: 旧行の追加列がNULLなら次に読むまで不表示。読んだ後に内容や並びが変わっても次の記録までは保存した比率を表示。旧版へ戻して読んだときに新列の古い値が残っても許容する。移行失敗はログとメーター非表示で扱い、従来の位置復元は維持する。proof JSON、trigger、内容版・manifest、監視、未開封本の方向解決、可視範囲worker、これらに付随する世代管理は採用しない。

前提のコード上の注意: 保存済み `page` はページ番号ではなくraw `items` index。HUDには要求どおりの読めるページ列を使う計算があり (§1.5)、これを再利用できる。一方Remoteの検証結果にはordinal/totalはあるが右綴じ情報が無い (§6)。初版は合意されたNULL保存を選ぶ。backlog §2.2 の「右上/右下未着手」は現行layoutと異なるため、現行 `ThumbnailOverlayLayout` を基準に下端の帯だけを足す (§5)。合意案を実施不能にする前提矛盾は見つかっていない。

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
| 詳細行・seek strip・Remote Web一覧・合成ビュー専用表示 | 今回の描画変更の対象外。通常物理一覧のFolder/ZipFile/PdfFileセルに限定 |

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

App全体が `HashMap<NormalizedResumeKey, ReadingMeterValue>` を1つ持つ。初回読込前/初期化失敗はmapのOptionがNone、利用可能時はSomeとし、別の準備完了/失敗boolを足さない。main/detachedのitemsやnavigation cacheにmapを複製しない。各viewerで記録する共通入口が同じmapを更新する。キーは既存book_resumeと同じDriveStripped正規化で、別ドライブ同名pathの衝突も現行仕様を引き継ぐ。

既存 `BookResumeWriter` に全行読込command/結果を加え、新規workerを作らない。起動時は移行後にSELECTを1回行い、追加3列が有効な行のmapを構築してUIへ返す。初回のmapがまだ無くても一覧は通常表示し、到着後にrepaintする。初回待機中の記録は§4.3の差分へ保持し、移行/初回SELECT成功後のsnapshotに反映して初めて描く。初期化失敗ではNoneのままで、raw記録は続けてもmeterだけは出さない。UI側はDBを読まない。表示OFFでも起動時の1回読込と記録時更新を行い、ONへ戻す際の追加loadを不要にする。

### 4.2 書き込み経路別の同期方法

| 経路 | mapの最小更新 |
| --- | --- |
| ローカル記録 | 同じ受付入口でkeyをinsert、補助値無しならremove。writerへ同じpayload |
| Remote記録 | §6のNULL記録を同じ入口へ渡し、そのkeyをremove。record_resume=falseなら変更無し |
| 読書位置の全件クリア | 既存writerへClear commandとして直列化し、map.clearとdedup解除を同じ受付で行う。結果で件数/メッセージを更新。UIでclear_all/countしない |
| deleteのstore purge完了 | 成功pathのexact / 配下 / `::` scopeに対応するmap keyをremove。既存metadata cache整理の完了点に接続。失敗/一部purgeの結果が不確かな場合は下記全行再読込 |
| rename/moveのstore移行・回復完了 | completion時に旧/新scopeの表示cacheを無効化し、writerで全行再読込。既存重複keyの扱いをUIで別実装しない |
| storeのcopy / 内容identityからの復元 | DB転記完了後だけ全行再読込。コピー先既存行優先などproduction SQLの結果を採用。通常file copyがresume DBを変更しないならこのための再読込は不要 |
| 設定リセット/復元・データストア再接続 | resume DBをクリア/置換/再接続する経路ならmapもclearして、新接続workerで全行再読込。メーターcheckboxのdefault/resetだけならmapを消さない |

rename/copyの全列移行は既存store registryを使用し、追加3列も一緒に運ぶ。稀な操作後の全行SELECTは許容し、独自prefix移行SQLや新しい差分DB監視は作らない。移行中は影響scopeの古いcell値を出さず、結果でrepaintする。他viewerのitems/worker/選択は変更しない。起動時の既存移行回復・後続cleanup完了も同じ再読込入口へ接続する。

### 4.3 非同期の全行読込に必要な最小限の相関

初回/再読込の古いsnapshotが、その待機中に記録した最新値やClearを上書きしないことだけを扱う。1つのpending receiverに **読込要求後のmap更新 (Set/Remove/Clear/RemoveScope)** を保持し、到着したsnapshotへ受付順に適用してからmapを置き換える。同じpending中の更新は表示mapにも直ちに適用する。これらはmap操作だけで、content/読み順のproofではない。

全行読込commandは既存writerのFIFOで、それまでのRecord/Clear後に実行する。外部store移行の完了後に要求するためその確定DBを読む。待機中に別の稀な操作が完了したら最新の全行読込を要求し、古いreceiverを捨てる。新要求より前のRecord/Clearはwriter FIFO、新要求後の更新はpending差分で守られる。receiverを分けるのでrequest世代・folder/view/range世代を追加しない。アプリ終了はreceiverを捨て、writerの既存終了drainを維持する。UIでjoin/waitしない。

map更新はAppの単一受付へ集約し、起動読込中・再読込中のSet/Remove/Clear/RemoveScopeを全て通す。比較的稀な操作のscope整理がmap全件を走査しても、毎frameの描画には入れない。新しいrollback・resume・supersession状態機械やretry loopは不要。

### 4.4 描画ホットパス

可視Folder/ZipFile/PdfFileのkeyをmapで引くだけ。既存itemsのpathからの正規化は毎paintで文字列allocateせず、メーター用keyをcell単位で初回に作ってidxにmemoする。既存のitems置換/削除に伴うidx状態破棄へこのmemoを接続し、保存したpath/keyを別cellへ流用しない。filter/sortではitemsの同じidxに紐付けたkeyを保持可能。全items分を初回にscan/cloneする必要はない。

保存keyの再計算はitems差替え/新cell描画時だけ。通常frameは可視セル数×key参照/map lookup/矩形描画程度。スクロールworker・本ごとの監視・catalog接続・GPU texture生成はゼロ。起動/稀なDB再読込の完了時と既存の記録/設定確定時にrepaintし、メーター取得のためのidle heartbeatを作らない。

## 5. 描画・方向・設定

### 5.1 下端の専用領域 (前案を維持)

`layout_cell_overlays` は `rect.shrink(4)` のinnerと実測badgeを純layoutへ渡し、`draw_cell` は同じlayoutをcell内へclipする (`src/app/grid_paint.rs:254`, `src/app/grid_paint.rs:383`)。現行 `ThumbnailOverlayLayout` はcheck/stack/top-left/bottom-left/filter count/media durationを所有する (`src/thumb_overlay_layout.rs:184`, `src/thumb_overlay_layout.rs:426`)。ここにmeter rectを追加し、下端帯の所有を1箇所にする。backlog §2.2の古い未着手記録を理由に四隅を再実装しない。

inner下端の高さ3 logical pt、左右はinner端、上に2pt gapを初期値とする。meterがある時だけ `badges_inner.max.y = meter.top - 2pt` として既存layoutへ予約を渡す。形式/フォルダ名/filename/評価、右下filter countはその上に置く。左上編集/pin/tag/time/UP、右上check/stackは既存優先規則を維持。動画/音声cellは非対象で、長さ表示と共存しないが共有duration layoutの回帰も確認する。極小cellはbadgeを優先しmeterを省略してよい。

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

ordinal/totalは既に得られるが、Remoteの閲覧directionは別のcontainer payload/settings経路にあり、今のwrite handoffの検証済みtupleに無い。本件でdirection再解決・session state・wire fieldを足さず、**Remote producerはraw indexを従来どおり記録し、追加3列は全てNULL、mapの当該keyはremove**とする (`src/remote_ipc/ui.rs:2754`)。本体の無関係な `App.reading_direction` や過去mapのrtlを流用しない。Remoteで読んだ本のmeterはローカルで再び読むまで消える、という合意済みの見え方を採用する。非root ZIP等のrecord_resume=falseでは従来どおり保存せず、過去root行を消さない。

本体egui一覧だけの描画変更であり、IPC wire/Web DOM/CSS/HTTP thumbnailにmeterを追加しない。**protocol 63を維持** (`crates/remote-ipc/src/lib.rs:31`)。Remoteのraw復元・履歴・bookmarkは既存動作を保ち、remote-webにDB writerを作らない。NULL対応の共通record入口へ接続するcore内部変更だけ。

## 7. 失敗の割り切り・未決事項

利用者の上記判断は確定として扱い、再承認を求めない。移行/初回SELECTが失敗したらログ、メーターを不表示、raw復元を維持。範囲/監視の再試行や復旧機械を作らない。通常の未読/NULL/0枚でnoticeを出さない。

DB書込失敗・稀な再読込失敗については次を設計責任者/利用者判断事項として残す。実装のためにretry/journalを設計し始めない。

- 記録のDB書込が失敗したとき: 推奨は既存writerのログに加えて既存notice経路で1回案内し、mapには受付済みの最新値を残す。画面は今回の値を表示できるが、再起動後は保存済みの旧値/不表示へ戻る。raw位置も保存に失敗する既存条件であり、データを削除しない。
- rename/copy等の全行再読込が失敗したとき: 推奨は影響scopeのmeterを非表示のままにし、ログと再起動案内。map全体の信頼を失うストア置換なら全meter非表示。他scopeは継続可能。読書位置を初期化しない。

通常の操作でmap更新を取りこぼさない接続と、読込snapshotが最新記録/Clearを上書きしない§4.3は通常の正しさとして実装する。追加の復旧対応が要るまれな組合せは条件と見え方を追記して相談する。独立レビューはこの簡単な設計の範囲を確認し、実装はその後の同じ実装セッションで行う。

## 8. テスト計画・ドキュメント更新・実装時のgate

| 対象 | 検証内容 |
| --- | --- |
| 移行/後方互換 | 出荷済2列DBに3列追加、旧path/page保持・NULL、再起動の冪等性、新DB、移行失敗でraw記録/復元維持。旧版SELECT/page-only upsertが動き、新列が残る了承済み挙動。新版NULL保存は3列を消す |
| 記録/HUD共有 | helperとHUDの列/位置が同じ。Image/Video/Folder/ZipImage/PdfPageの混在、filter・通常/詳細sort・stack flat・本扱いOFF・製本順。raw idxとordinalの違い、anchor無し、0/1枚・途中・最後・後戻り、RTL Single、見開き/連結/Split/補助表紙。HUDに相手pageが出てもanchorだけ記録 |
| 記録更新 | raw/3列の同時upsert、同idxでもtotal/ordinal/rtl/NULLが変わればdedupしない。読んで一覧へ戻るとmapが即時最新、OFFでも記録維持。内容/並びが変わっても読み直し前は保存値のまま |
| map初回/再読込 | 初回worker全行read、NULL/不正値非表示、読込中のSet/Remove/Clear/RemoveScopeを古いsnapshotが上書きしない。再読込receiver差替え、起動回復、別viewer記録で共通map更新・他context不変 |
| 他のDB変更 | Remoteで当該key削除、非記録Remoteは不変。delete/rename/move/copy/内容identity復元のproduction DB結果とmapの一致。prefix境界、コピー先既存行優先、Clear後の再記録、DB再接続/reset。無関係なscopeを消さない |
| 対象セル/比率 | Folder/ZipFile/PdfFileだけ。変換書庫/Stack/個別page等は無し、旧NULL/行無し/total0はtrackも無し。1/N・途中・N/N、FollowReading×保存rtl、LeftToRight、方向設定変更は再read無し |
| layout/snapshot | meterとbadge rect非交差、cell_h/scroll/sort/hit-test不変。Light/Dark・最小幅/普通幅・DPI・選択/check・編集/tag/pin・形式/長名/評価/filter件数、cut opacity、動画長さ回帰。純fixtureとPreferences説明をsnapshot |
| 設定の通し | 実PreferencesStateで編集→本番OK helper→保存→DB再load→開き直し。ON/OFF・方向保持、Cancel不変、欠落field/default/resetはON、favorite/runtime列設定mergeを破壊しない |
| 速度/Remote | 通常paintからDB/FS/列挙へ到達しない。可視key memoを別idxへ誤流用しない。scrollしても全行SELECT/worker/thread/watchが増えず、idle repaint loop無し。IPC63/wire fixture・Remote raw復元/履歴/bookmark不変 |

実装時に更新する文書は `docs/spec.md` と `htdocs/mimageviewer/` の設定/読書位置説明 (過去に保存した比率、旧NULL、内容変更、Remote後の非表示)、`docs/architecture-overview.md` のbook_resume列/メモリmap、`docs/async-architecture.md` の既存writer全行read、`docs/display-pipeline.md` の下端帯、`docs/README.md` の索引、本設計の実装記録。backlog §1.256は合意仕様と実装状態へ更新し、§2.2は今回の帯追加と現行レーンの状況を整合させる。今回の段ではこの設計書だけを書き換え、完了扱いにしない。

実装後は絞り込みlib/integration/snapshotから検証し、共有変更の最終gateは `scripts/test-full.ps1`、`cargo fmt --check`、UI文字変更の `python scripts/check_ui_glyphs.py`。その後 `scripts/build-dev.ps1` で利用者用binaryを作り、通常profileの注意と具体的確認手順を渡す。今回の文書改訂ではbuild/test/起動しない。

将来の利用者確認は混在Folder/PDF/ZIP・途中/最後/後戻り・左右方向/見開き・読んで戻る・ON/OFF・設定保存・内容変更後の保持・Remote後の非表示。agentによるlive確認を実施するなら、disposable portableと時間/desktop操作の範囲を提示して明示承認後だけ行う。通常profile binaryはagentが起動しない。
