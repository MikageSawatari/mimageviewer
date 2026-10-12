# 全文索引の欠落・取り込み失敗対策 (§1.367)

2026-10-11 設計 r3。製品コード調査基点: `next-prefs` = `master`、
`0f6ad9f191d5a5d25f175d9b5f49611afbfc1489`。利用者がr1を`a0927b26e`、r2を`4c86015de`でcommitした。
r1の独立設計レビューはREVISE (P1なし、P2 3件、P3 1件)。r2の再レビューでは前回指摘を解消し、
正常終了による未実行merge taskの打切りを通知/再構築から除外する新規P2 1件でREVISEとなった。
r3はその境界と回帰を明記し、名前索引の別件をバックログ§1.369へ採録した。
利用者がr3を`02c2b4c4b`でcommitし、独立再レビューのACCEPTを伝えた。設計改訂自体は
製品コードの変更・製品ビルド・mIV起動を含まない。受理後の実装と検証は§12へ追記する。

## 1. 範囲と守る条件

利用者決定: 深刻な問題として次のリリースで修正する。起動点検は起動時間を延ばさない。
費用が起動に影響する場合は起動待ちから外すか、点検を見送る。

- 索引の書き込み失敗を成功・「初回作成完了」と表示しない。ログと通知を必ず残す。
- Tantivy commit + reader reloadの成功より先にSQLiteのOkを公開しない。
- 索引を破棄する前にSQLite inventory resetとdurable pendingを同じtransactionで確定する。
- 起動点検のためにFS走査、全文文書の列挙、全filesの追加走査を増やさない。
- UI threadにI/Oや待機を持ち込まず、正常時の閲覧・検索・監視・手動確認を維持する。
- 作り直す対象は再生成可能な`fts_meta.db`のinventoryと`fts_index/`だけ。
  設定・tags.db・評価・コレクション・本の構成・しおり・原画像を消さない。

**まれな失敗は割り切り (ログ + 通知 + 次回起動で作り直し)。**
一般のingest/commit再試行、途中候補の保存、rollback/resume、失敗分類による回復分岐、
回復失敗の回復は設計しない。後述のWindows atomic persistだけが、限定された例外案。

## 2. 証拠と未確定事項

### 2.1 原事故の保存コピー

提供された`C:/home/mimageviewer/target/fts-incident-2026-10-10/`を読んだ。
SQLiteは`mode=ro&immutable=1`で照会し、通常profileのDBは開いていない。
本調査の読み取りスクリプトと結果:
`target/fts-integrity-investigation/inspect_incident.py`、`incident-and-cost.txt`。

- `files`: 840,464行、全行`status=0` (Ok)。`index_state`は空。
- 保存された`meta.json`: 2 segments、生存文書数399
  (`max_doc - deletes.num_deleted_docs`の和)。
- §1.367の先行調査 (ClaudeCode/Codex) は、399文書が18:04:55以降の更新行と完全一致したと記録する。
  本調査は行数・segment数・生存数を再集計した。399件のpath集合の再比較はしていない。
- 欠落した瞬間のログ・ファイル操作traceは無い。原事故の原因は未特定。

### 2.2 夜間復旧のログ

利用者指定の通常ログは読んで即座にtargetへコピーした。
調査時には夜間分が`%APPDATA%/mimageviewer/logs/mimageviewer.log.bak`へrotate済みだったため、
そちらもコピーした。継続的に元ログを開いてはいない。
保存先: `target/fts-integrity-investigation/overnight-mimageviewer.log[.bak]`、
`overnight-indexer-excerpt.txt` (コピーの行番号付き)。

| .bakの行 | process経過秒 | favorite UUID | 記録 |
| --- | ---: | --- | --- |
| 14366 | 1096.598 | d501796c-ca9a-4d51-a52c-7c1ec476b8bf | ingest apply failed、IO/os error 5 |
| 14745 | 1126.264 | d2ab1ce6-9344-4b1d-ba7f-72e7b57118c8 | 同上 |
| 40579 | 3128.385 | a4ee8071-7046-4ff5-9c4b-642ab18f2cf6 | 同上 |
| 60327 | 5956.971 | d2ab1ce6-9344-4b1d-ba7f-72e7b57118c8 | 同上 (追加の4回目) |
| 79123 | 7420.388 | d501796c-ca9a-4d51-a52c-7c1ec476b8bf | 同上 (追加の5回目) |
| 109150 | 13800.970 | d501796c-ca9a-4d51-a52c-7c1ec476b8bf | upsert apply failed、同じIO/os error 5 |

最初の3回の壁時計時刻00:07:48 / 00:08:18 / 00:41:40と、件数が止まり
「監視中」のまま通知が無かったことは、§1.367のClaudeCode観測。
ログ自体の時刻はprocess相対秒なので、追加の3件へ壁時計時刻を推測で付けない。

先行観測の`rebuild-progress.tsv`も読んだ。00:16以降にTantivyがSQLiteより200〜300件多い
区間がある。01:09からmeta.jsonを読まないSQLite-only観測へ変更した後も失敗が増えた。
このため「10分ごとのmeta.json読み取りだけが原因」とは言えない。
02:3xに846,352行となり、インストール版Ctrl+G `klee`で10,000件へ戻ったことは
同TSV末尾のClaudeCode観測。エージェントによるmIV実行の観測ではない。

### 2.3 この調査で行ったstandalone試験

製品を起動せず、使い捨て`target/fts-integrity-investigation/stress-data/`のみへ書いた。
既存debug依存rlibのTantivy 0.26.1をrustcでリンクした。製品コード・Cargo依存は変更していない。
ソース`stress.rs`、結果`stress-result.txt`。

- reader `OnCommitWithDelay` + commitごとの明示reload + 10個の古いmmap searcher snapshot保持で、
  2,000 commits / 40,000 delete+upserts。100,341msでアクセス拒否なし。
  これは小さいschema/データの有界試験であり、実規模・Defender・ACL・全interleavingの保証ではない。
- `meta.json`を`FILE_SHARE_READ | FILE_SHARE_WRITE`だけで開き、削除共有を故意に禁止すると、
  `MmapDirectory::atomic_write`が`Os { code: 5, kind: PermissionDenied }`を返した。
  旧metaのbyte列は不変。
- 同じhandle保持中の`IndexWriter::commit`は夜間と同じ
  `An IO error occurred: 'アクセスが拒否されました。 (os error 5)'`を返した。
- handle解放後、同じ内容のatomic replacementは成功。
  エラー後のIndexWriter::commit再試行はしていない。

補助試験`atomic_retry_probe.rs` / `atomic-retry-result.txt`では、write/flush/syncを済ませた
**同じTempPathのpersistだけ**を最大4回 (待ち10/25/50ms) 試した。
15ms後に妨害handleを閉じる場合は3回目・38msで成功し、永続妨害では88msでerrorを返し旧内容を保持した。
これはatomic-level案の根拠であり、製品にその実装があるという意味ではない。

## 3. 戻り値の経路: どこがアクセス拒否を返し得るか

Tantivyの参照はCargo registry
`C:/Users/mikag/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tantivy-0.26.1/src/`
を基準とする。旧名`directory/mmap_directory.rs`は、この版では
`directory/mmap_directory/mod.rs`と`file_watcher.rs`に分割されている。

### 3.1 mIV側

`src/ingest_worker.rs:148`のflushはdispatcherへ
`batch_cancellable(... commit=true, reload=true)`を送り、errorを`?`で返す。
`src/fts_writer_dispatcher.rs:462`の`process_batch`はdelete→upsert→commit→reloadの順。
SQLite upsert/deleteの失敗は`src/ingest_worker.rs:189,198`で`SystemError`に文脈付きで変換するので、
今回の文脈なし`IoError`という表示はSQLiteエラーではない。
メタ抽出失敗も`:281`でlogして候補をskipするため、今回のfull apply errorとは別。

`src/fts_index.rs:457`のupsertはdelete_term→add_document。
add_documentのキュー送信失敗はTantivy側の`ErrorInThread`になる。
生のIO errorは主にcommit内部のI/Oから返り得る。現ログにphase/pathは無く、
commitと後続のreloadを事後に一意に識別できる情報は無い。

### 3.2 Tantivy側の候補表

| 操作 | コードと伝播 | 判定 |
| --- | --- | --- |
| commitのmeta.json公開 | `indexer/index_writer.rs:618,664` prepare/join→PreparedCommit→`segment_updater.rs:442` schedule_commit→purge_deletes→segment_manager.commit→save_metas。`:38,416`で`directory.atomic_write(meta.json)`の生IOを返す | **有力候補。制御試験で同じerrorを得た**。夜間の実操作は未特定 |
| meta.json atomic replace | `mmap_directory/mod.rs:354` temp作成→write→flush→sync_data→TempPath.persist。`tempfile-3.27.0/src/file/imp/windows.rs:92` persistはSetFileAttributesW→MoveFileExW(REPLACE_EXISTING)、失敗を生IOで返す | 置換の妨害はos5になり得る。temp作成/write/syncも同じ生IO型なので現ログだけでpersistまで断定不可 |
| .managed.json公開 | `managed_directory.rs:51,205` save_managed_paths / register_file_as_managed。同じatomic_writeを使う。segment open_write経由なら`:286`でOpenWriteError (path付き) に包む | 単独の可能性はあるが、通常のsegment登録失敗なら今回と表示が異なる。meta登録済みならcommit時の再登録はno-op |
| segment生成・delete bitsetの書込/flush/sync | prepare_commitのworker joinがworker errorを返す。`mmap_directory/mod.rs:334` SafeFileWriter::terminate_refのflush/sync_dataも生IO。schedule_commitはpurge_deletesのerrorを返す | commit errorの候補。segment open失敗はOpenRead/OpenWriteError、書込み終端の生IOは排除できない |
| GCによるsegment削除 | `managed_directory.rs:164`でdelete失敗をfailed_to_delete_filesへ収集。Windowsのmmap削除失敗は想定済み。`segment_updater.rs:452`のGC Resultも無視する | **この削除失敗そのものが今回のapply errorを返す経路ではない**。GC内.managed公開errorもcommitでは無視される |
| merge | `segment_updater.rs:666`でend_mergeのtask内からsave_metas。mergeは別worker/Resultで、commitは全merge完了を待たない。`:610`の自動mergeはFutureResultをdropする | 今回のapply errorへの直結は未証明だが、**mergeの最終書込・公開失敗はdispatcherのjob Resultでは拾えない**。§6.1のper-writer通知hookで共通出口へ接続する |
| 明示reader reload | `src/fts_index.rs:451`→`reader/mod.rs:241,287`。`:202`でMETA_LOCK、load_metas、segment reader open | dispatcherからは返る。ファイルopen/lockの拒否なら通常はpath/lock付きerror。内部での生IO可能性はphase計装で切り分ける |
| watcher自身のopen/reload | `mmap_directory/file_watcher.rs:50,83`で500msごとFile::open→checksum。失敗はloop内で処理。`reader/mod.rs:82` callbackのreload失敗もlogだけ | **watcher error自身はapplyへ返らない**。間接的な共有妨害の有無は下記 |

### 3.3 Windowsの共有条件と仮説の強さ

Rust標準`File::open`は既定でREAD/WRITE/DELETEを共有する
([Rust公式OpenOptionsExt](https://doc.rust-lang.org/std/os/windows/fs/trait.OpenOptionsExt.html))。
Tantivy watcher、atomic_read、open_mmapはこれを使い、削除共有を外していない。
watcherはmeta.jsonをmmapせずchecksumの間だけ通常read handleを保持する。
従って「Tantivy watcherがFILE_SHARE_DELETEを付けていないため失敗」はコードと一致しない。
通常reader/watchの試験も失敗していない。watcher停止やManual reloadへの変更を対策にはしない。

Windowsでは削除共有なしのopenがrename/deleteを妨げる
([Microsoft CreateFile](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilea))。
本機の制御試験ではmeta replacementがos5を返した。
Defender等の外部scanner、別の観測tool、ACL/readonly、temp側の妨害は仮説であり、
当夜のhandle/操作traceが無い以上、どれも実原因として断定しない。
metaの読取監視をやめた後にも失敗しており、その観測toolへの単独帰属は支持されない。
また、mmap segmentのGC失敗とmeta replacementの拒否は別の操作である。

## 4. なぜ静かに止まり、元の欠落は直らないか

### 4.1 Fullの失敗で候補が消える経路 (コードで確定)

1. `src/indexer_supervisor.rs:706`でFull outcomeをFailedにして開始する。
2. `:810`のsession.applyがErrなら`:824`でlogし、run_initial_scanからreturnする。
   apply内の`to_ingest.into_iter()`もdropされ、まだ処理していない候補を保持しない。
3. `:736`のRAII guardは`in_full_scan=false`へ戻す。外側`:446`は成功/失敗に関係なく
   `initial_scan_done=true`とし、`:472`でprogressをclear、監視loopへ進む。
4. 既存のFailed outcomeは残るが、お気に入りUIの`draw_state_inline`
   (`src/ui_dialogs/favorites_editor.rs:1557,1575`) は
   `(in_full_scan, initial_scan_done)`だけを見て「監視中」にする。
   自動Full再投入も通知も無い。watcherは残り、以後の変更だけを処理する。
5. [今すぐ確認]は既存Fullを再要求するため、SQLiteに無い残りの候補を再発見できる。

同型のfailure出口: watcher Remove、Upsert、候補なしUpsert→Remove
(`src/indexer_supervisor.rs:973,1030,1054`周辺)、sidecar再取り込み(`:1101`)。
後者はerrorをlogして次の画像へ進むだけ。Fullにはinitial/manual/overflow/watch回復の入口がある。
構成変更cleanupは既に`metadata_reconfiguration.rs:197`の
`cleanup_failure`でdurable pending+通知へ進む。ingestのfailureもこの方針へ揃える。

### 4.2 同じapply経路は「SQLite Ok、Tantivyなし」を作れるか

**限定を付けてYes。ただし、夜間のmeta replacement拒否だけが原事故を作った証明は無い。**

- 新規upsertのcommit失敗は、そのbatchのSQLite Okを新たに書かない。
  単にFullが止まっただけなら、両ストアは未完の件数になる。
  atomic meta replacementが失敗した制御試験では旧metaは保持された。
  これだけでは「過去84万件だけ消え、新しい399件だけ残る」を説明しない。
- deleteのTantivy commitが成功し、reader reloadが失敗すれば、applyはSQLite deleteへ進まない。
  **既存Ok行が残り、Tantivyでは削除済み**になる。SQLite delete自体の失敗でも同様。
  後者のerror表示はSystemErrorなので、今回のos5へ混同しない。
- upsertでも既存Ok行を変更前に残したまま失敗し得る。batch/worker/mergeの途中状態は
  アプリ側から包括的に保証できず、一般commit再試行を安全だと仮定しない。
- さらに**現在の再作成の順序は確実な穴**:
  `FtsIndex::open_at` (`src/fts_index.rs:365`) はmissing/staleならTantivyを先に作り直し、
  `open_stores_with_rebuild_sync_using` (`src/indexer_manager.rs:221`) が後からfilesをresetする。
  その間の中断では新しい空Tantivy + 古いOkが残り得る。
  runtime requestの`request_item_index_rebuild` (`src/fts_meta.rs:470`) はmarkerとscan印だけを変更する。
  このpendingを処理する`:181`のwipeも、files resetより前である。
  semantic version移行自体はfiles再作成とpendingを同じtransactionに置いており、ここは別扱い。
- `src/fts_index.rs:812`はwipe失敗をlogして続行する。残骸のmetaをcreateで置換し得るため、
  正常な空directoryという前提も保証していない。

原事故のこの再作成gapへの帰属は仮説。当日のschema検出/wipe/中断の証拠は残っていない。

### 4.3 補修を見逃す経路 (コードで確定)

`src/indexer_manager.rs:714`のreconciliationはnot-Ok行だけを読む。
`src/search_walker.rs:311,356`はFSとSQLiteのみを比較し、Ok+所有+署名一致ならunchanged。
Tantivy存在照会はしない。そのため既存Okの文書欠落はFullや再起動だけでは拾われない。
Tantivy-only文書も全件照合しない。現在の「3-way diff」はadded/changed/deletedの3分類であり、
FS/Tantivy/SQLiteという3ストアを照合する実装ではない。

## 5. Windows atomic-level対策と診断

### 5.1 推奨案: persistだけの有界再試行

本機の制御試験を根拠に、Tantivy 0.26.1の小さなローカルpatchとして、
`mmap_directory::atomic_write`の**TempPath.persistだけ**をWindowsで限定再試行する案を推奨する。
現在のregistry実装にはatomic persistの再試行は無い。
Tantivyのlock取得retryとは別であり、「upstreamにある既知の対策」とは主張しない。

- 同directoryにtempを1回作成し、write/flush/syncを1回済ませる。
- persistで生のWindows code 5 / 32 / 33が返った場合だけ、同じTempPathと同じdest/contentで
  最大4 attempts、待ち10/25/50ms (合計85ms)。OS操作時間は別なので壁時計上限とは言わない。
- `PathPersistError.path`を回収して再persistする。失敗でtempをdropして次回へ渡さない。
  既存destをdelete/truncateしない。非Windowsと正常系は従来の単発操作。
- 5は永続的なACL拒否にもなる。5を「必ず一時的」と分類せず、4回で通常errorへ戻す。
- temp作成/write/flush/sync、segment write、reader reload、全commit、whole ingestは再試行しない。
- registryキャッシュを書き換えず、version/checksumの基点を固定したvendor patch + Cargo patchで管理する。
  更新範囲・license・ローカル差分を記録する。依存versionを便乗更新しない。

安全性の根拠: 失敗したpersistは旧metaを保持し、成功時だけ完成済みbyte列を原子的に公開する。
同じtempの置換を繰り返す間にdocument/delete queueやSegmentManagerを再実行しない。
ManagedDirectoryの登録・segment updaterの直列化もそのまま。
commitの上位では内部stateがsave_metas前に変更される (`segment_updater.rs:450`) ため、
dispatcherでcommitを丸ごと再試行する代案は採らない。

これは**共有妨害への限定mitigation**であり、夜間errorの全原因を直したとの完了条件にはしない。
独立設計レビューでこの例外の安全性を確認する。実原因が別段と判明すれば、そこで直し、
他段へのretry拡大はしない。Defender除外の要求やwatcher停止で機能を変えない。

### 5.2 不足する計装を埋める

dispatcherはupsert/commit/reloadのphase、favoriteまたはrequest、batch件数、
Tantivy errorのDisplay/Debugをfailure時だけ記録する。
atomic patchはtemp-create/write/flush/sync/persistの段、dest path、raw_os_error、attemptを記録する。
IO errorを文字列だけへ変換せずraw codeを保持する。persist errorはmeta.jsonと.managed.jsonを区別する。
成功全件の同期log追加はしない。回復した有界persistは集約counterまたは1 recordで足りる。

これにより次の発生では「commitが失敗」から「meta.jsonのMoveFileEx系persist失敗」等へ絞れる。
外部の実handle所有者は依然別traceが必要。製品内にprocess探索・Defender判別は入れない。

## 6. ingest/commit失敗時の出口

共通出口は**ログ + 通知 + durable request_item_index_rebuild**。
既存`metadata_reconfiguration::cleanup_failure`と同じmanager-owned通知経路
(`src/indexer_manager.rs:526` take_notifications、`src/app.rs:30384` toast)を使う。
各supervisorへ独立したrecovery workerを作らない。

1. 共通failure reporterをmanagerがdispatcher/runtimeの開始前に用意する。
   正本metaと同じ通知viewを参照し、独立したrecovery state/workerは持たない。
   dispatcher jobのTantivy書込み/reload errorは**dispatcherがreply送信前に**報告する。
   Batch/Commit/favorite purge/path-range purgeの全jobを通す。
   jobと独立した自動mergeの最終書込・公開errorは§6.1のhookから同じreporterへ渡す。
   session.apply後のSQLite errorも同じreporterへ渡す。
   Full・watcher upsert/remove/fallback・sidecarでローカルlogだけへ戻さず、同じ出口を使う。
   メタ抽出で特定1ファイルを読めない場合のskip、明示cancelは全索引破損と混同しない。
   cancelでcallerがreplyを放棄した後のcommit failureもdispatcher側で拾う。
   SQL errorはoriginで1回報告し、dispatcher由来errorをsupervisorでも二重通知しない。
2. workerで既存pendingを書き、scan印を失効させる。runtimeではfilesやTantivyをwipeしない。
3. 「索引の更新に失敗しました。mIV を再起動すると索引を作り直します。」を通知する。
   pending書込自体が失敗したときもlog+通知は残すが、再構築予約成功とは言わない。
   その失敗の再試行・別marker・自動DB作り直しは足さない。
4. 通知だけではtray中に見逃すため、お気に入り編集にも索引全体の「再起動で再構築」状態を残す。
   manager workerが既存durable markerの状態を短いviewへ投影し、UIはそのsnapshotだけを読む。
   新しいper-favorite pending/failed bool群は作らず、markerを正本とする単一のmaintenance表示にする。
   FullのFailed outcomeもdraw_state_inlineへ渡し「監視中 = 初回作成完了」と誤表示しない。
   watcherは動いていることと取り込みの成功を別の表示意味にする。
5. watcherと既存[今すぐ確認]は維持する。現在sessionで残りを読み直せてもpendingを消さない。
   完全整合の証明が無いのでpending clearは次回のcross-store作り直し成功だけ。
   failure後の候補を自動でFullへ再投入し続けるloopは作らない。

close/dropもinventoryを照合した。`src/indexer_manager.rs:643`のDropはbackground finalizerへ
commitを送り、そのResultを`:653`で捨てている。dispatcherをfailure報告ownerにすれば、
このcommitもcallerの寿命に依存せずlog/pendingへ進む。
終了後に見せられないtoastのために終了を待たせず、pendingを処理する次の起動で
「索引を作り直しています」を示す。background finalizerへmeta/reporting handleの寿命を保持する。
reporter→view→dispatcherのArc cycleを作らず、DB lockを持ってwriter reply待ちをしない。

Tantivyを共有するので再構築予約はfavorite単位ではなく索引全体。
既存cleanupも同じ対象である。失敗時に新たなworker再起動/rollback、writer poisonの解除、
全windowのモーダル化は追加しない。稀な書込failure後の途中stateは次回作り直しで解消する。
既存の監視・検索・手動再確認を一律停止する機能制限は採用していない。

### 6.1 自動merge・非同期save_metasの最終失敗

自動mergeは`segment_updater.rs:610`で`drop(self.start_merge(...))`する。
`start_merge`のmerge workerは`:550`で`end_merge`のResultをFutureへ送るだけであり、
`:666`のsave_metas最終失敗もこのResultへ入る。dispatcherのcommit Resultと別なので、
callerの存在や次のcommitを待つ通知では漏れる。

vendor patchに**IndexWriterごとの最終merge failure hook**を用意し、mIVはraw writerを
dispatcherへ渡す前に同じ共通reporterを登録する。process-global callbackは使わない。
`start_merge`のworkerが返すmerge/writeまたは**実行された**end_merge/save_metasの最終Errを、
Futureへ送る前・そのconsumerの有無に関係なくhookへ1回報告する。
既存のpersist最大4回を使い切ったerrorと、persist以外の書込errorを含める。
merge候補が開始前に無効になっただけの`segment_manager.start_merge`失敗は書込失敗として扱わない。
merge完了時に元segment集合が利用できなくなった`SegmentManager::end_merge`の拒否も、
既存診断logだけを残し通知/pendingの対象外とする。commitが全件削除済みsegmentを取り除く
通常のupsertでも発生する。判定はこの発生箇所のprivate typed outcomeを使い、
`InvalidArgument`全体やerror文字列で除外しない。根拠・回帰は§12.4。
同期commit/save_metasはdispatcherから報告し、このhookと二重報告しない。

**終了による未実行taskの打切りを、書込失敗から除外する。** コード上の事実として、
`index_writer.rs:807`の正常Dropも`:809`で`segment_updater.kill()`を呼ぶ。
merge自体が進行中でも、その後の`end_merge` (`segment_updater.rs:615,624`) が
`schedule_task`へ来た時にupdaterがkilledなら、`:335–336`はtaskを実行せず
`SystemError("Segment updater killed")`を返す。このErrで通知/pendingを立てない。
開始前の無効なmerge候補とは別の、merge開始後に起こる正常な打切りである。

識別に依存するのは**schedulerが当該taskを受理しなかった分岐の構造化した原因**。
Tantivy 0.26.1の`TantivyError` (`error.rs:57,102`)には専用のcancel/killed variantが無く、
現在は汎用SystemErrorの文字列へ潰される。vendor patchではprivateなscheduler入口を
`Result<FutureResult<T>, UpdaterKilledBeforeSchedule>`として、このtyped admission signalを
同じ`:335`の判定で生成する。既存`schedule_task`は必要なら従来Resultへのadapterとし、
`end_merge`からmerge workerまではこの未実行原因と実行後のResultをflattenせず渡す。
hookは`UpdaterKilledBeforeSchedule`だけを除外し、その後で既存Future consumer向けの
Resultへ変換する。mIVへ新しい終了stateやfailure分類を追加するための型ではない。
messageの一致/部分一致、汎用SystemErrorの一括除外、事前の別`is_alive()`チェック、
hook実行時のkilled/shutdown flagには依存しない。schedulerの判定と別チェックの間のraceを作らない。

merge本体のI/O失敗、受理/実行されたend_mergeのadvance_deletes/save_metas等の最終Errは、
その後にwriterがdropされた場合も既存log+通知+durable pendingへ渡す。
未実行taskの打切りと、終了中に実際のI/Oが返したerrorを混同しない。
`save_metas` (`:388`) が既存のis_alive分岐で書込みを省きOkを返す場合は元からfailureではないが、
書込み開始後のerrorまで終了理由で抑止しない。他のSystemErrorもこの除外には含めない。
merge完了待ち、追加retry、打切りからの復旧処理は増やさない。

hookはmergeまたは`end_merge` (内部のtask.waitを含む) が**戻った後**に呼ぶ。
atomic_write内、ManagedDirectoryの登録lock内、segment updaterのtask内でDB/reportingを呼ばない。
Directory/segment管理lockを解放したterminal Result境界から、共通reporterが
log→durable request→短い通知viewへ進む。worker上の処理でありUIのDB待ちを増やさない。
reporter内部もDB lockとview lockを同時保持せず、writer/merge完了を待たない。
hookの保管lockはclone取得直後に解放し、callback実行中に保持しない。

callbackはmetaと通知viewだけを保持し、Index/FtsIndex/IndexWriter/dispatcherやjoin handleを
捕まえない。writer/merge jobの寿命に沿って利用でき、終了後のfailureも§6のfinalizer方針へ進む。
通常UIを終了後の通知のために延命しない。merge再試行、次commit待ち、rollback/resume、
別のfailure workerや回復queueは追加しない。

**通常のmmap segmentのGC削除拒否はhookの対象外。**
`managed_directory.rs:164`のfailed_to_delete_filesは使用中segmentの通常の回収保留であり、
merge/writeまたはend_merge/save_metasのterminal Errとは別である。
hookはGCのfailed_to_delete_filesの件数や通常GC logへ反応しない。
§5.2のatomic診断はmerge originも記録し、最終errorの通知経路はこのhookが担当する。

### 6.2 名前/コンテナ索引・類似索引の対応範囲

同型経路をコードで棚卸しした。名前索引は`search_index.db`単独であり、FTS reporterの
`fts_meta.db` markerでは復旧しない。本件のscopeを名前索引の全書込処理へ広げず、
**共通UIのFull結果表示だけを今回合わせ、名前索引の通知/再構築接続は別件へ引き継ぐ**。
FTS対策で名前索引も全て閉じたとは記録しない。

| 名前索引の入口/出口 | 現行コードと問題 | 今回のリリースでの扱い |
| --- | --- | --- |
| initial Full、手動Full ([今すぐ確認])、watcher overflow Full | `name_index_supervisor.rs:429,327,351`→`:443` run_full_scan。`:513`はFailedでもinitial_scan_done=true、last_full_scanにはtyped outcomeを残す | **表示を修正する**。`favorites_editor.rs:814`が既存NameFullScanOutcomeを渡し、Failed/Cancelledを作成完了の「監視中」と表示しない。処理開始/終了というinitial_scan_doneの役割は保つ |
| Fullのprepare_full_scan/child upsert/prune失敗 | `name_index_supervisor.rs:468`と`name_bulk_indexer.rs:224,251`。BulkSummary.had_errorにはFS観測失敗とDB書込失敗が混在し、Full outcomeはFailed | **別件**。今回は上行の表示を直すだけ。DB書込failureのlog+通知+名前索引marker接続は別backlog。全had_errorをDB破損とみなさない |
| Full成功後の完走印write失敗 | `name_index_supervisor.rs:553` record_complete_scan失敗はlogのみ、次回Fullで再確認される | **別件**の通知棚卸しへ含める。本文書ではrebuildが必要と機械的に決めない |
| watcherでmissing root/ancestor/subtreeの削除 | `name_index_supervisor.rs:665,668,689,725`のdelete_subtree失敗はlogのみ。apply側`:636`はupdates_appliedを増やす | **別件**。名前索引の共通failure出口と成功件数の訂正を引き継ぐ |
| watcherのparent refresh、subtree upsert、post-scan prune | `name_index_supervisor.rs:874,973,790`の書込。FS観測失敗/DB失敗はlogやErroredへ進み、通知/markerへは揃っていない | **別件**。read不能とDB書込失敗を区別し、既存の不完全観測時prune抑止を維持して対応する |
| OFF/削除/root変更時のclear | `name_index_manager.rs:143`はclear失敗で`:145`のrequest_rebuild_on_next_startを呼ぶ | **既存対応を維持**。今回FTS markerへ置換しない。名前索引の既存clear-failure回帰を再利用する |

名前索引の別件は既存`SearchIndexDb::request_rebuild_on_next_start` (`search_index_db.rs:472`) と
次回writable openの`rebuild_names_if_requested` (`:944`) を使う方針で引き継ぐ。
新しいretry/途中state保持を要求しない。marker設定や回復そのものの失敗は既定のlog+通知で閉じる。
別件は[次リリース検討バックログ](next-release-backlog.md)の**§1.369**へ採録した。
着手/リリース優先度は管理担当が別件として決める。本件のFTS修正の受入条件へ
名前索引の全failure対応を混ぜない。

2026-10-12: §1.369の通知所有、停止後の通知保持、全入口の共通出口と成功件数の具体化は
[name-index-write-failure-plan.md](name-index-write-failure-plan.md)を参照。事前独立設計レビュー待ちで、
名前索引の製品コードはまだ変更していない。新規retry/回復workerは設計に含めない。

類似索引はSQLite正本から`similar_search_array::load_or_rebuild` / `rebuild_from_sqlite`
(`src/similar_search_array.rs:166,195`)で派生検索配列を再生成できる。
お気に入りUIも`favorites_editor.rs:1329,1344`でFailed/Degradedを表示する。
FTSの「inventory reset前にTantivyを破棄した」構造とは異なるため、SQLite正本の破棄や
FTS rebuild順序を機械的に適用しない。今回の変更対象に含めない。

## 7. 起動時整合点検: 起動待ちへ入れない

### 7.1 費用の実測と不採用案

事故コピー840,464行をPython/SQLiteで読み、各queryを3回計測した。
Rust製品の起動実測ではない。SQL行読取時間で、Rust ownership判定は含まない。
tableの初回readに真のcold cacheは保証できない。結果は`incident-and-cost.txt`。

| query | EXPLAIN | 実測ms |
| --- | --- | --- |
| 現行SELECT path,favorite_id | COVERING INDEX idx_files_fav_path | 615.233 / 417.711 / 427.003 |
| status列を加えたSELECT | SCAN files、coveringを失う | 614.117 / 611.594 / 627.232 |
| COUNT WHERE status=0 | SCAN files | 166.475 / 164.618 / 156.142 |
| COUNT WHERE status!=0 | COVERING INDEX idx_files_status、0行 | 初回0.234、以後0.001〜0.018 |

全Ok COUNTやstatus列追加は不採用。statusをcovering化する追加index/migrationも作らない。
「COUNTは常にO(1)」「workerなので起動時間0」とは言わない。

### 7.2 推奨する位置と集計

`metadata_reconfiguration::startup_cleanup` (`src/metadata_reconfiguration.rs`) の
既存所有走査が返す`owner_rows`を再利用する。`FtsMetaDb::for_each_path_owner`
(`src/fts_meta.rs:735`) のSELECTは変えない。
別途、全favoriteの`COUNT(*) WHERE status != 0`を部分indexで取得し、
`ok_rows = owner_rows - not_ok_rows`とする。statusがOk/Failedの2値である前提はopen時migrationと
テストで固定する。お気に入り集合で限定しない (orphanも全索引比較に含める)。

Tantivy側は**既にロード済みの同じSearcher**の`num_docs()`を使う
(`core/searcher.rs:123`、segmentごとの生存数を合計、追加I/O無し、O(segments))。
max_docの合計やdirectoryのファイル数を使わない。

比較は**startup cleanupのdelete/commitおよびFailed reconciliationの前**、supervisor spawn前。
その時点でruntimeに取り込みproducerが無く、FS watcher applyや構成変更もまだ実行しない。
同じworkerの固定configurationでSQLite集計とreader snapshotを採る。
既存startup走査結果のcleanupを先に実行し、後から古い行数を比較する順序にはしない。
writer init/reader openをもう一度行わず、点検のためのcommit/reloadはしない。

このworkerは`Runtime::start` (`metadata_reconfiguration.rs:94`) でspawnされ、
`IndexerManager::new_with_stores` (`indexer_manager.rs:405`) はその完了をawaitしない。
Appの初期化result送信 (`app.rs:29113`) とoverlay解除 (`:30079`) の条件へ点検を加えない。
UIが追加SQLを実行したり、点検completion待ちを追加したりしない。
追加作業量は既存行数counterの再利用 + Failed部分indexの集計 + segment数合計。
Failedが大量のときはその件数分の費用はあり、固定ゼロmsとは保証できない。
先行perfコピーの`perf_events.3.jsonl:112,115`にも、manager_new 4.0497msの後に
ownership cleanup 186.2317ms (840,461行) が別threadで完了した記録がある。
これは既存の非同期位置の証拠であり、新点検後の起動費用の検証ではない。

### 7.3 判定、復旧、限界

- `live_docs < ok_rows`なら欠落の疑いとして数値・時間をlog、通知、既存pendingへ進む。
  supervisor/readerが既にmanagerから公開され得るため、**このsessionでwipeしない**。
  次回起動に§8の共通再構築経路で両方を作り直す。live rebuild/reopen機構を作らない。
- `live_docs >= ok_rows`は「件数不足を検出しなかった」であって整合証明ではない。
  Tantivy-only/重複が欠落を相殺し得る。path単位の照合は今回追加しない。
- reader snapshotとSQLite間の通常ingest windowではTantivyが多くなり得るため、
  supervisor spawn後に単純比較する案は採らない。
- `skip_offline_change_scan`を使う正常利用者へ新しいFullを強制しない。
  異常検出時だけ既存pendingの規則へ進む。点検のためにINDEX_VERSIONを上げない。

### 7.4 起動点検の採用判断 (2026-10-11)

当初の R3 は cold/warm × skip ON/OFF × Failed 0/少数/大量 × idle/I/O競合の24条件で、
各3 pair以上の A/B を採用条件としていた。**2026-10-11、利用者が軽量手順 option 1 を承認し、
設計リードがその手順で測定して採用を決定した。** 旧24条件 gate の完了とは区別する。
実装者が独自に条件を緩めたり、microbenchmark だけで採用したりしたものではない。

測定 source は `63aa26dbd771af43ad8f99497925844f59fc6386`、profile は `dev-runtime`。
A は default build (点検 OFF)、B は当時の `--features fts-startup-integrity` build (点検 ON)。
exe の SHA256 は A=`388aa2351ce0dbe1a56e52f05de169b31d4263f4dd550523ace943292352b80c`、
B=`72b2942dc050f66ca00a2df85705e5df81371cc8aaf3fb03ab475d010138112c`。
agent 作成データから用意した隔離 seed に4お気に入り (item index ON、
`skip_offline_change_scan=false`)、SQLite Ok 16,385行と一致するTantivy生存文書を持たせ、
各 run 前に seed を新しくコピーした。warm cache、idle、AB → BA → AB → BA → AB の10起動
(各arm5回)。通常 profile/事故原本は使っていない。起動・測定は設計リードが実施し、
今回の実装者は保存済み記録を読み取り照合しただけで、mIV を起動していない。

| perf session 起点からの時刻/区間の中央値 (秒) | A: 点検 OFF | B: 点検 ON |
| --- | ---: | ---: |
| `first_frame` | 0.619 | 0.602 |
| `metadata_supervisors_started` | 0.631 | 0.617 |
| `indexer_overlay_cleared` (UI-ready) | 0.774 | 0.729 |
| supervisor開始 − first_frame | 0.012 | 0.014 |

B の `integrity_check_ms` は 0.0479–0.2105ms、全5回で `missing_suspected=false`、
owner/Ok/live件数は16,385、not_ok=0。全10ログの保存診断は既存 `validate_delivery` で complete。
原記録は `target/fts-ab/ab-results.json` と `target/fts-ab/run-XX-*/logs/perf_events.jsonl`。
中央値・hash・件数・保存診断の照合は `target/I-1367-adoption-measurement-verification.json`。

設計リードは差をノイズと判断し、測定可能な起動費用の増加はないとの結論と、§12.1/§12.2の
大量Failed fixture (追加COUNT+num_docs約0.1–15ms、UI待ちへ入れない配置) を合わせて採用した。
**製品では点検を常時実行し、feature gate を撤去して一つの経路にする。** perf event は維持する。
これはこの測定条件での採用判断であり、cold、skip ON、大量Failed、I/O競合のlive A/Bや
release profile の計測、全環境での数学的な追加時間ゼロの証明ではない。worker間競合の可能性、
Failed件数による集計費用、件数照合の限界は残る。起動barrier、全件照会、新index/migrationは追加しない。

## 8. 再構築順序を1つにする

runtimeでのrequest (markerのみ) とstartupでの実行 (inventory reset+wipe/create) を区別する。
実行ownerは`open_stores_with_rebuild_sync`に集約し、
`FtsIndex::open_at`単体が勝手に旧索引を破棄する契約を廃止する。

1. SQLite open/migration。meta欠落+Tantivy残存、既存pendingも現在の検出を保つ。
2. **SQLiteのdurable pendingがある場合は、旧Tantivyをprobe/open/parseせず手順3へ直行する。**
   meta.jsonや.managed.jsonが読めない/壊れていても、その成功をwipeの前提にしない。
   pendingが無い場合だけTantivyを非破壊でprobeし、compatibleをそのまま再利用するか
   missing/staleを返す。そのprobeのread/parse errorはerrorであってmissingではない。
   読めない保存SQLite DBを勝手に消さない。pendingの読取失敗も「pending無し」と扱わない。
3. 作り直しが必要ならSQLite transactionでfiles reset、scanned_once clear、pending=1を一括commit。
   既存pending (旧実装がfilesをまだ消していない場合も含む) も必ずこの手順を通す。
4. pending無しのprobeが持つ旧Index/directory handleがあればdropしてからTantivy dirをwipe。
   pending有りでは旧索引を開くhandleを初めから作らない。
   NotFound以外のremove errorは中止する。残骸へcreateして続行しない。
5. 新しいdirectory/index/readerをcreate/open。成功後だけpending clear。
6. 通常のmanager worker cleanup/reconciliation→supervisor spawnへ渡す。
   全scan印はreset済みなので、skip設定でも必要なFullが動く。

version移行のfiles再作成+pendingという既存transactionを維持する。
schema mismatch、fts_index不在/meta.json不在、meta DB不在、semantic version migration、
ingest/cleanup/integrityからのpendingのすべてをこの順序へ合わせる。
`reset_inventory_for_recreated_tantivy`は「再作成後」ではなく「破棄前」の共通操作へ組み直す。
direct FtsIndex利用のテスト/helperも破壊契約を明示し、通常アプリの別入口で順序を迂回しない。

途中終了後に空Tantivy+旧Okが残るgapを所有境界で閉じる。
wipe/create/pending clearの失敗はlog+通知で終え、markerを残す既存の次回起動入口を使う。
バックアップ複製、transaction跨ぎrollback、再試行schedulerは追加しない。

## 9. 文書訂正と簡素化の判断

実装と同時に以下を訂正する。本調査ではこれらの既存文書を書き換えていない。

- `search-architecture.md` §4.1/4.2/4.3、§6.2/6.3、§9:
  FS/Tantivy/SQLite「3-way diff」、SQLite失敗はlogだけ、[今すぐ確認]で全欠落を補修する、
  reconciliationを同期実行、という過大/旧記述を修正。
  FS↔SQLiteの3分類、count点検の限界と位置、次回再構築、再構築のdurable順序を明記。
- `fts_meta.rs`冒頭、`ingest_worker.rs`冒頭とflushコメント、indexer supervisorの古いwriter Mutex説明:
  同じ過大な復旧保証と現在のdispatcher所有に合わせる。
- `async-architecture.md`のmetadata worker表: 現在のdispatcherとsupervisor内の逐次applyへ訂正。
- お気に入り編集の説明/仕様/検索マニュアル:
  FTSと名前索引の両方で監視開始とFull成功を区別し、FTS再構築予約の通知・状態表示を説明する。
  名前索引のDB書込failure通知/再構築は§6.2の別件が未実装だと明記し、修正済み扱いにしない。
- §1.367へ本設計と検証結果をリンクし、原事故の原因未特定を残す。
  夜間の追加失敗を、原事故が同じ原因だったという記録へ書き換えない。

簡素化: background全件取込をモーダル化すると閲覧/検索が長時間使えなくなるので不採用。
既存pendingに乗せ、runtimeのlive wipe/reopenと途中候補保存をなくす。
通常操作の機能を削らず、回復の入口は再起動1つにする。
全正常利用者のversion bump/rebuild、per-path integrity DB、retry taxonomyも作らない。

## 10. 実装と検証のまとまり

設計owner/実装owner/独立reviewerはAGENTS.mdの分担で手配する。
この調査はdesigner/investigatorのbounded handoffで、追加のcoordination teamは作っていない。
重要なstartup/runtime所有境界、非同期merge hookの寿命/lock境界とatomic例外は
実装前の独立レビュー対象。r3ではr2のREVISEを受けて未実行taskの打切り境界を改訂し、再レビューへ渡す。

### 10.1 必須の回帰

1. atomic persist: 削除共有なしhandleでos5、短時間解放後に成功、永久拒否で有界error・旧meta不変。
   write/sync等の他段errorは再試行しない。非Windows/正常系は単発。meta/.managedを区別するlog。
   **実際にpatched Tantivyをリンクしたon-disk E2Eを必須にする。** 初回commitで旧文書群を作り、
   次のcommitに新規追加・既存更新(delete+add)・明示削除・無変更文書を混在させる。
   meta.jsonのpersist直前hookで削除共有なしhandleを開き、retry予算内に解放する。
   上位commitを1回だけ呼んで成功を得る。全writer/readerをdropして再openし、
   生存path集合、文書内容、削除結果、重複なしを期待集合と照合する。
   atomic byte一致だけやstandaloneのpersist模倣では代用しない。
2. commit/reload/SQLite delete/upsert各failure注入:
   commit+reload失敗で新規Okを公開しない。既存Ok + Tantivy delete成功 + reload失敗の形を作り、
   通知/pendingと次回再構築で復旧。一般のcommit再試行や自動Fullループが無いこと。
3. Full initial/manual/overflow/watch回復、watcher upsert/remove/fallback、sidecarから
   同じfailure出口へ到達するhandler-level coverage。cancelや1ファイルのbuild failureは区別する。
   replyを放棄したcaller、finalizer commit、両purgeのerrorでもlog/pendingを残す。
   自動mergeを実際に発生させ、そのmergeのmeta公開persistへ恒久妨害を注入する。
   Future consumerを持たず後続commitも呼ばない条件で、最大4 attempts後の最終Errが
   共通reporterへ1回届き、通知+durable pendingが残ること。DB/view/Directory lockの
   reentrancyやArc cycleが無いこと。通常mmap snapshot保持中のGC削除拒否では通知/pending無し。
   **自動merge開始後にwriterをdropする回帰**をpatched Tantivy経由で追加する。
   自動mergeの開始をtest gateで確認し、end_mergeのscheduler受理前でmerge workerを留める。
   writerをdropしてkillを完了させた後gateを解放し、未実行原因が
   UpdaterKilledBeforeScheduleとして返ること、通知0件・pending無しを確認する。
   Future consumer/後続commit無しでも同じ結果であること。worker終了を観測する待ちはtest内だけで、
   製品Dropへmerge完了待ちを追加しない。初期fixtureのpendingは無しとし、別failureを混ぜない。
   対になる回帰として、mergeの実I/Oまたは受理されたend_mergeのpersistが開始済みの状態で
   test gateに留め、writer drop後にそのI/Oを最終失敗させる (persistなら既存4 attempts)。
   killedでも実行由来Errはlog+通知1件+durable pendingへ届くこと。
   文字列だけ同じSystemErrorを実行由来Errとして注入する場合も除外されないことを確認する。
4. UI: Failed Fullを「初回作成完了」にしない。trayでtoastを見逃しても編集画面にpending表示。
   正常完走と正常監視のsnapshotは維持。UIがDBを直接読まない。
   同じ表示入口を使う名前索引も既存NameFullScanOutcome::Failed/Cancelledを渡すhandler/snapshot回帰。
   完走・省略・再Full開始の表示を維持し、FTS markerを名前索引へ適用しない。
5. startup count: 840k対399相当、全空、正常一致、Failed混在、orphan、delete済segmentのalive数。
   countは全files、Partial Failed index使用と既存owner covering scanをEXPLAINで固定。
   aggregateへ余分な全件scan/Vecを入れない。cleanup前のsnapshotを使用する。
   多いdocsは欠落なし保証にしない (重複相殺の限界をfixtureで示す)。
6. rebuild: missing directory/meta、stale schema、pending、semantic migration、missing meta DB。
   destructive-operation hook直前にfiles=0/scanned_once=0/pending=1がdurableであること。
   中断を各checkpointで注入し再openで完走。wipe拒否でcreate/openしない。
   最終create/open失敗とpending clear失敗も既存markerを保持する。
   **pending + 読めない/壊れた旧meta.jsonまたは.managed.json**でprobeが0回、reset+pending確定→
   wipe/createへ直接進む回帰を追加する。解析不能fixtureとprobeが呼ばれれば失敗するhookを使う。
   file-read拒否の実handle試験では、wipeまで恒久妨害する条件とprobe不要の条件を区別する。
   pending無しの同じread/parse errorはmissing扱いにせず、非破壊でerrorを返すこと。
7. user data: tags/ratings/bookmarks等の別DB fixtureをhash比較し変更なし。
   正常favorite ownership cleanup・root変更・OFF→ON・skip印・pause/cancel/shutdownを維持。
8. cost: 84万行以上のコピーfixtureでSQL plan/時間とRust worker追加時間を記録。
   UI-ready/result-sendへcompletion barrierを入れない。
   当初の24条件採用gateは、利用者承認の軽量手順と設計リードの採用判断へ変更した。
   測定範囲と未測定条件を区別し、§7.4の記録を正本とする。

既存回帰は`ingest_worker::reader_reload_failure_prevents_sqlite_publication`、
`sqlite_failure_is_reported_and_allows_later_success`、
`metadata_reconfiguration::cleanup_failure_sets_rebuild_pending_and_notifies_without_runtime_wipe`、
`indexer_manager::recreated_fts_store_resets_matching_inventory_and_restores_docs_same_startup`、
missing-meta/wipe-retry/migration系を拡張する。
「稀な回復失敗の組合せ」を新機構で塞ぐテスト要求にはしない。ログ/通知/markerという契約を検証する。

### 10.2 実装後のgateと担当

実装者がnarrow lib (fts_meta/fts_index/dispatcher/ingest/manager/supervisor/reconfiguration)、
`search_metadata_e2e`を先に実行。UI変更のsnapshotとglyph lintを実行し、
共有writer/startup変更なので最終`scripts/test-full.ps1`、fmt/check、`scripts/build-dev.ps1`まで担当する。
独立reviewerは同じ結果を再実行せず証拠を確認し、残る重要経路を追加確認する。
本r3は設計のみのため製品test/build gateの成功や利用者確認用binaryを成果として主張しない。

liveの承認をこの調査から推定しない。実装後の大規模再取込/起動A/Bは、別途具体的なscenario・時間・
desktop入力・使い捨てportable範囲を提示して承認を得るか、利用者自身の手動確認へ渡す。
normal profileの実行は利用者だけ。既存事故copyはread-only、通常settingsは実験に使わない。

## 11. 利用者の判断が必要な項目

**現時点の追加質問は無し。** 次の版への採用・起動時間を延ばさない条件は決定済み。
まれな書込failureの通知+次回再構築、retryをatomic操作に限定する技術判断、
点検のSQL/worker配置について利用者へ稀なfailure別の選択質問を作らない。
検証で正常起動が遅くなれば決定済み条件に従い点検を見送る。
この採用条件の変更と採用決定は§7.4に記録した。以後の製品点検は常時実行する。
通常の機能制限やuser-created dataの破棄が必要と判明した場合だけ、該当項目ごとに
推奨案・失われる挙動/データ・費用を示して別途判断を求める。

## 12. 実装記録 (2026-10-11、受理済み R3 本文は維持)

以下の§12冒頭〜§12.2は初回実装・追加検証時の記録。**最新の採用状態は§7.4と§12.3**。
当初の opt-in/default OFF と24条件手順は、利用者承認の軽量A/B後の採用判断で更新した。

初回実装対象は `02c2b4c4b` からの差分で、利用者が `bee3d4aa0` にコミット済み。
ローカル Tantivy 0.26.1 は実ファイルとして
`vendor/tantivy` へ固定し、完成済み TempPath の Windows persist だけを最大4回まで再実行する。
上位の document/delete/commit は再実行しない。由来・原版 checksum・変更箇所は
`vendor/tantivy/MIV-PATCH.md` に記録した。通常 build は `miv-test-hooks` を含まない。

同期 dispatcher job、SQL 更新、非同期 merge/end_merge の最終失敗は、manager-owned
`FailureReporter` の log + 通知 + 既存 durable pending へ接続した。後続成功では pending を
解除せず、実行中の索引を消さない。正常 cancel は typed `ScanError::Cancelled`、正常終了で
未実行 end_merge が拒否された場合は patched scheduler の private typed
`UpdaterKilledBeforeSchedule` で区別する。文字列や「終了中」の一括判定を使わず、終了中に
実行された I/O/同文言 SystemError は報告する。GC の削除拒否もこの報告対象に含めない。

再作成は `open_stores_with_rebuild_sync` へ集約し、SQLite inventory/scan印 reset + pending を
FULL synchronous transaction で先に確定する。旧 handle の drop、wipe/create/open、pending clear
はその後に行う。既存 pending なら旧 meta.json/.managed.json の probe を省く。
wipe/create/clear の失敗に再回復を追加せず、marker と既存の起動 error を維持する。

お気に入り編集は FTS/名前索引の既存 typed Full outcome を表示へ射影し、Failed を正常な
監視開始フラグで隠さない。FTS の共有 maintenance snapshot をダイアログ全体へ表示し、
SQL/I/O/待機を UI へ追加しない。正常完走/正常省略/再 Full開始を維持する。
名前索引の実書込 failure 通知/marker 接続は §1.369 の別件として残す。
`search-architecture.md`、`async-architecture.md`、`ui-responsiveness.md`、仕様/検索マニュアルの
FS ↔ SQLite 差分の限界、再構築の durable 順序、非同期境界と通知の説明も訂正した。

点検は `fts-startup-integrity` opt-in・既定 OFF。既存 ownership scan の件数と loaded reader を
再利用する worker 上の実装で、OFF build に追加 COUNT/比較の呼出しはない。
起動 A/B 手順 (撤去済み、§12.3) と `scripts/analyze_fts_startup_ab.py` は、隔離 data-dir、
24条件、交互反復、保存 checkpoint、全 endpoint と count 診断を確認する。
未測定/欠落/ノイズ/判断不能/悪化は不採用。起動 A/B、遅延なしの証明、点検採用、原事故の
原因確定は未実施であり、非対話 fixture の費用測定を採用証拠へ読み替えない。

独立実装レビューと追加の実 persist 拒否 ingest 回帰レビューは PASS。
UI snapshot は baseline 生成後の目視確認まで実施し、文字欠け/切れなし。解析器の Python
unit test 8件と UI glyph lint zero は成功。Rust/full/build と fixture の最終検証結果は
`target/I-1367-impl-msg.txt` にコマンド、件数、ログとともに記録する。

通常構成の fmt、core check、`cargo check --features portable`、narrow lib/integration と
最終 `test-full.ps1` は成功。全体ログは13,274 passed/0 failed/63 ignoredで、子 process の
同じ test 再実行2件を含む。workspace外 vendor も25/11/18件成功。`build-dev.ps1 -PreserveRuntime`
は通常 feature の core/remote/EPUB worker とPE検査まで成功し、実アプリは起動していない。
先行失敗と最終実行条件も引き継ぎ記録へ残し、全体 gate を起動性能の証拠に使わない。

初回引き継ぎでは、起動点検 feature 付きの追加検証が環境 block のため未完了だった。
解析器の初期テストで作成した
`scripts/tmp26xigyp9` を Cargo が列挙できず、OS access denied で compile 前に止まった。
agent 側のACL照会/修復・非再帰削除も失敗し、通常 PowerShell での対象だけの環境整理を依頼した。
現在の解析器テストは在メモリ fixture で、この directory を新たに作らない。
Cargo 設定で拒否を隠さず、利用者の対象 directory 整理後に §12.2 の検証を完了した。
正常構成の helper/fixture の成功を、この feature 入口の成功へ読み替えない。
点検は未採用・既定 OFF のまま。初回の block 記録は引き継ぎファイルに保持する。

### 12.1 非対話 fixture の費用 (採用判定ではない)

正常構成の lib test 実行体から ignored test
`metadata_reconfiguration::tests::startup_integrity_large_copy_fixture_cost` を直接実行し、1件成功。
`cfg(test)` でコンパイルされた同じ COUNT/num_docs helper と840,464行の正常対応を使った。
各 Failed population は1回、全体ゲートと同時実行であり、idle/cold/warm は固定していない。
`target/I-1367-startup-fixture-cost.log` に原記録を保存した。

| Failed 行 | 生存文書 | segments | ownership SQL scan (ms) | 追加 COUNT + num_docs (ms) |
| ---: | ---: | ---: | ---: | ---: |
| 0 | 840,464 | 6 | 204.676 | 1.788 |
| 84 | 840,380 | 6 | 207.897 | 0.221 |
| 756,418 | 84,046 | 4 | 206.575 | 15.141 |

ownership SQL scan は空 callback の streaming 読取で、実製品の UUID/所有範囲判定全体の時間ではない。
追加時間はこの fixture/実行時の実測値であり、壁時計上限や起動 ready の無影響を保証しない。
feature付き実行経路の検証とも区別し、§7.4の起動 A/B 条件はそのまま維持する。

### 12.2 起動点検 feature の検証追補 (2026-10-11)

利用者が `scripts/tmp26xigyp9` の ACL reset/削除と実装の `bee3d4aa0` へのコミットを完了した後、
残っていた feature ON の検証を完了した。以下の追補差分は未コミット。
新しい一時 directory は system temp または `target/` だけを使い、実アプリは起動していない。

最初の feature ON cleanup 回帰では2件失敗した。正常 fixture が SQLite Ok 行だけを作り、
対応する Tantivy 文書を作っていなかったため、点検が正しく欠落を報告し、scan 印と通知数の
前提を変えたもの。正常な ownership/cleanup fixture を両ストアが一致する状態へ修正し、
通知無し/pending無しと文書保持・削除も確認した。意図的な欠落 fixture、期待値、timeout、
製品の検出条件は緩めていない。`metadata_reconfiguration.rs` の変更は `#[cfg(test)]` module
内のみで、module より前の製品 source が HEAD と同一であることも照合した。

解析器の CLI 確認では `runs: null` が未捕捉 TypeError になったため、配列でない `runs` を
理由付き `not_adopt` (exit 1) にする入力検証と回帰を追加した。実ファイルの合成 perf log
144本/24条件を CLI へ渡し、正常同時間・B遅延・checkpoint欠落・不正runsの4ケースを確認した。
stdout と `--output` の JSON も一致した。これは解析器の確認であり、起動計測の証拠ではない。

| 検証 | 結果 | 原ログ (`target/` 内) |
| --- | --- | --- |
| feature ON `--lib startup_integrity` | 4 passed / 0 failed / 1 ignored | `I-1367-startup-feature-verification.log` |
| feature ON core check (test hook無し) | exit 0 | `I-1367-startup-feature-core-check.log` |
| feature ON `--lib metadata_reconfiguration` (修正後) | 23 passed / 0 failed / 1 ignored | `I-1367-startup-feature-reconfiguration.log` |
| feature ON `--lib indexer_manager` | 21 passed / 0 failed | `I-1367-startup-feature-manager.log` |
| feature OFF `--lib metadata_reconfiguration` (修正後) | 22 passed / 0 failed / 1 ignored | `I-1367-followup-default-reconfiguration.log` |
| feature ON ignored 840,464行 fixture | 1 passed / 0 failed | `I-1367-startup-feature-fixture-cost.log` |
| 解析器 Python unit test | 9 passed | `I-1367-ab-analyzer-reverification.log` |
| 合成ログでの解析器 CLI | 4ケース成功 (採用証拠ではない) | `I-1367-ab-cli-smoke.log` |
| fmt / diff whitespace check | exit 0 | `I-1367-followup-fmt.log` / `I-1367-followup-diff-check.log` |

filter 同士の重複は合計しない。先行の cleanup 21 passed / 2 failed / 1 ignored は
`I-1367-startup-feature-reconfiguration-before.log` に保存した。先行の不正runs CLI の traceback は
`I-1367-ab-invalid-runs-before.log`、修正後の拒否結果は `I-1367-ab-invalid-runs-after.log` に保存した。
変更がテスト・解析器・文書のみのため、製品 runtime が変わっていない通常構成の full/portable/
確認用 build は §12 の有効な結果を再利用し、今回の追加成功を全体 gate へ加算しない。

feature ON の Cargo lib test 実行体から同じ ignored test を直接実行した費用は次の通り。
各1回、feature OFF の test compile と同時実行で、idle/cold/warm の統制はない。

| Failed 行 | 生存文書 | segments | ownership SQL scan (ms) | 追加 COUNT + num_docs (ms) |
| ---: | ---: | ---: | ---: | ---: |
| 0 | 840,464 | 7 | 189.411 | 0.093 |
| 84 | 840,380 | 7 | 185.937 | 0.082 |
| 756,418 | 84,046 | 5 | 174.818 | 11.625 |

§12.1 と同じく SQL scan は空 callback の streaming 読取。これを起動時間の上限、A/B、
UI-readyへの無影響と読み替えない。点検は未採用・既定 OFF。利用者の隔離 A/B は
計測手順 (撤去済み、§12.3) に従う、という追加検証時点の判断だった。
その後の利用者承認の手順変更・採用は§7.4と§12.3に記録する。

### 12.3 起動点検の採用とfeature撤去 (2026-10-11)

利用者承認の軽量A/Bを設計リードが実施し、§7.4の証拠とfixture費用を確認して製品への採用を
決定した。実装者はその決定に従い、`63aa26dbd` を基点に `fts-startup-integrity` を
Cargo feature一覧から削除し、`IntegrityCounts`、`FtsMetaDb::count_not_ok`、startup呼出しと
cleanup前比較の回帰を通常buildの単一経路にした。runtime toggle/設定は追加していない。

既存worker、所有行数、同じloaded Searcher、reporterのlog+通知+既存pendingを維持する。
UI待ち、追加全件走査、索引作成、commit/reload、回復worker/retryは追加しない。
perfの `fts_integrity_check`、`metadata_out_of_range_cleanup`、`metadata_supervisors_started` と
件数・費用を維持し、既存のfeature判別属性 (`feature_enabled` / `integrity_feature_enabled`) は
過去ログとの比較用に常にtrueを出す。異常時の通知文そのものは既存reporterの文面を使う。
通常のmmap GC拒否・正常shutdown取消の扱いを変えず、patched Tantivyの回帰も確認する。

旧24条件向け専用解析器 `scripts/analyze_fts_startup_ab.py` とそのunit testを削除した。
`docs/fts-startup-ab-measurement.md` と解析器は採用後に削除し、記録は本書 §7.4 / §12.3 に一本化した。
過去の検証台帳と `target/fts-ab/` の原証拠は保持し、旧解析器が軽量手順をACCEPTしたとは
主張しない。architecture/async/UI応答性/仕様とマニュアルを常時点検へ合わせた。
`docs/next-release-backlog.md` は利用者指示で今回編集しない。

今回の検証結果、件数、条件、原ログは `target/I-1367-impl-msg.txt` の採用追補へ記録する。
実アプリは起動せず、変更は未コミット。測定による採用判断と非対話コード検証は区別する。

| 通常buildでの採用後検証 | 結果 | 原ログ (`target/` 内) |
| --- | --- | --- |
| `--lib startup_integrity` | 4 passed / 0 failed / 1 ignored | `I-1367-adoption-startup-tests.log` |
| `--lib metadata_reconfiguration` | 23 passed / 0 failed / 1 ignored | `I-1367-adoption-metadata-tests.log` |
| `--lib indexer_manager` | 21 passed / 0 failed | `I-1367-adoption-manager-tests.log` |
| `--lib fts_` | 87 passed / 0 failed | `I-1367-adoption-fts-tests.log` |
| `--test fts_tantivy_integrity` | 11 passed / 0 failed (両shutdown回帰を含む) | `I-1367-adoption-tantivy-tests.log` |
| `--test search_metadata_e2e` | 16 passed / 0 failed | `I-1367-adoption-search-e2e.log` |
| core check / `cargo check --features portable` | exit 0 / exit 0 | `I-1367-adoption-core-check.log` / `I-1367-adoption-portable-check.log` |
| fmt / UI glyph lint | exit 0 / zero | `I-1367-adoption-fmt.log` / `I-1367-adoption-glyphs.log` |

狭いfilterと全体gateの重複は合計しない。ignored は大規模fixtureの任意実行であり、
同じ集計実装の先行費用測定 (§12.1/§12.2) は保持する。今回のコード検証を追加の起動A/Bと
読み替えない。

先行full gateの本体libは12,006 passed / 1 failed / 56 ignored。失敗は既存collectionの
`deleted_collection_child_stays_physical_through_park_resume_and_backspace` で、park前の履歴に
残った削除済みcollectionがresume時に正しくpruneされ、snapshot比較が `[] != [Collection]`
になった。削除表示の完了とready catalogによる履歴pruneは別通知であり、fixtureが固定回数の
pollで両者の完了を仮定していた。`ui_dialogs::collections::prune_collection_folder_history_from_ready_catalog`
の製品動作は変更せず、既存5秒の `poll_until` で削除表示とback/forward履歴のprune両方を
確認してからsnapshotするよう、`src/app/collection_grid.rs` のtest moduleだけを修正した。
park/resume/Backspaceのassertionとtimeoutは維持し、履歴を手動clearして失敗を隠していない。
先行ログは `I-1367-adoption-full-before.log`、製品source不変の照合は
`I-1367-adoption-collection-scope.log`。修正前の同じ実行体による単独1件成功も記録するが、
先行full失敗を成功と読み替えない。修正後に単独回帰と全体gateを確認する。

修正後の単独回帰は1 passed / 0 failed、最終 `test-full.ps1 -SuppressCrashDialogs` はPASS/exit 0。
全result recordの集計は13,275 passed / 0 failed / 63 ignored (子processの同じtest再実行2件を含む)、
本体libは12,007 passed / 0 failed / 56 ignored、UI snapshot integrationは157件成功。
workspace外vendor egui/egui-wgpu/eframeは25/11/18件成功。原ログ/集計は
`I-1367-adoption-full.log` / `I-1367-adoption-full-results.json`。
条件はjobs=1、threads=4、専用 `target/i1367-adoption-tmp`、assertion/timeout/全target保持。
修正したcollection回帰の成功後に確認用buildも並行した。実行条件は
`I-1367-adoption-full-conditions.txt`。これを起動性能の追加証拠にしない。

`build-dev.ps1 -PreserveRuntime` は通常featureのcore/remote/EPUB worker配置まで成功、exit 0。
core buildは10m13s、core SHA256は `2783ce2d661a0a8a9bee11b73a7124ca711e0c2885093eec3ef6296beefb6518`。
原ログとexe/DLL identityは `I-1367-adoption-build-dev.log` / `I-1367-adoption-dev-runtime-artifacts.json`。
portable/test hookは有効にせず、startup点検は通常コードとして含む。アプリは起動していない。

### 12.4 公開直前の無害なmerge拒否の誤通知修正 (2026-10-12)

利用者報告: 署名済みv4.6.0の実機で約33分に26回、SegmentManagerの元segment未検出と
`InvalidArgument("The segments that were merged could not be found ...")`が発生し、毎回
更新失敗のトーストと次回作り直し予約が出た。同時期に約5秒ごとの画像追加と約840件の
再索引/commitが観測された。この実機ログの回数・規模は利用者観測であり、本作業でアプリは
起動していない。別セッションの`docs/next-release-backlog.md`には触れず、commitもしない。

対象はCargo patchで固定した`vendor/tantivy` 0.26.1。修正後のsource位置:

- `src/fts_index.rs:473`のupsertはdelete_term→add_document。dispatcherの単一writerが
  delete/upsert→commitを実行する。製品`src/`にTantivy writerのrollback/delete_all_documentsの
  呼出しはなく、通常監視のcommitをこれらへ帰属させる根拠はない。
- `vendor/tantivy/src/indexer/segment_updater.rs:522`のschedule_commitはpurge_deletes→
  SegmentManager.commit→save_metas。save_metasはcommitted_segment_metasを取得し、
  `segment_manager.rs:132,228`で生存数0のsegmentをregisterから除去してmeta.jsonを公開する。
  mergeは別workerで走るため、その元segmentが全件upsertで置き換わる間にcommitできる。
- `segment_manager.rs:202`のend_mergeは元集合が全てcommittedまたは全てuncommittedに
  存在するかを最初に確認する。集合不成立の拒否はregisterのremove/addより前。
  `segment_updater.rs:786`もこの結果ならsave_metasへ進まない。従って最新commitを
  巻き戻したり削除したりせず、利用できないmerge結果を捨てるだけである。
  register混在も同じ集合不成立に含まれるが、正常に開始したmergeは単一registerから選ばれ、
  commitは全entryを一度に移すので、今回の通常競合の原因は元segmentの除去である。

修正はこの所有境界だけ。`MergeSourcesUnavailable`と`EndMergeOutcome`をprivateに保ち、
observerへ渡す本当のErrと区別してから既存公開FutureのInvalidArgumentへ変換する。
文字列一致・汎用InvalidArgument除外・終了flagの別判定は使わない。元のWARNは保持する。
merge本体、advance_deletes、save_metasのI/O/他のerrorは従来どおりreporterへ渡す。
dispatcher/reporterの作り直し予約・通知処理自体は変更しない。
既に記録されたpendingは本当の失敗との区別ができないため取り消さず、次回起動の既存処理に従う。
本作業は通常profileのDB/設定を読み書きしない。

並行組合せを減らす案も確認したが、commitをmerge待ちにしたり監視を止めたりする必要はない。
公開不能なmergeを捨てる既存構造を保持するので、modal化・retry・回復state・追加待機は不要。
独立Sol / xhighの事前設計レビュー・完了差分レビューともPASS (修正を要する指摘なし)。

回帰は実際のon-disk Tantivyと既存のper-writer test gateを使う。自動merge policyで開始し、
merge完了直前にworkerを止め、20回のdelete+add upsert/画像追加/commitを進める。
元segmentが全て消える場合と、変更しない文書を持つ元segmentが1個残る場合を検証する。
故意のrollback/delete_all/error注入でこの拒否を作らない。元IDの除去と実際のWARNを確認し、
各commit/merge拒否後/再openの件数・path→内容、scanned_once保持、pendingなし、トーストなしを
assertする。明示mergeの公開Future互換性、および同じ全文のInvalidArgumentを別箇所で返すと
本当の失敗として通知される試験も追加する。既存の実OS persist拒否の回帰も再実行する。

未commitの本節差分に対するWindowsでの検証 (全てexit 0):

| command / 対象 | 実数・結果 | 原ログ (`target/` 内) |
| --- | --- | --- |
| `cargo test -p mimageviewer --test fts_tantivy_integrity` | 14 passed / 0 failed / 0 ignored (新規3件) | `fts-merge-tantivy-tests.log` |
| `cargo test -p mimageviewer --lib fts_` | 87 passed / 0 failed / 0 ignored | `fts-merge-lib-tests.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功 (通常、test hookなし) | `fts-merge-normal-check.log` |
| 同core check `--features portable` | 成功 (test hookなし) | `fts-merge-portable-check.log` |
| `cargo fmt --all -- --check` / vendor変更3 Rust fileの`rustfmt --check` / `git diff --check` | 成功 | `fts-merge-fmt-check.log` |

合計101件成功。自動mergeの2条件は競合区間が各20 commits / 40 upserts / 20追加文書で、最終生存数
22件と23件を各commit・拒否後・再openで確認。明示mergeも同じ20 commitsの全消失条件を確認。
初回integration fixtureのcompileはcrate-privateなscanned_once helperの参照で失敗したため、
public APIを増やさずfixture専用SQLに修正してから上表を実行した。製品compile/test失敗ではない。
full gateと実アプリ検証は今回の指定範囲外。実機ログと同じ84万件での負荷試験を実施したという
意味ではない。

通常profileの`build-dev.ps1 -PreserveRuntime`もexit 0で完了 (core 5m16s、Remote 34.22s、
EPUB worker 0.39s)。core/Remote/EPUB workerと必要DLL/bundleの配置、VCRT PE検査まで成功。
portable/test-script/test hookは有効にせず、アプリは起動していない。原記録は
`target/fts-merge-build-dev.log`、exeのSHA256/size/timeは
`target/fts-merge-dev-runtime-artifacts.json`。通常 `%APPDATA%/mimageviewer` を使うため、
利用者が実行するときはinstalled/tray-residentを先に終了する。手動確認は索引作成完了後の
監視フォルダへの約5秒間隔の画像追加、最新検索内容の保持、誤トーストが出ないこと。
