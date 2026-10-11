# 全文索引の欠落・取り込み失敗対策 (§1.367)

2026-10-11 設計 r2。コード調査基点: `next-prefs` = `master`、
`0f6ad9f191d5a5d25f175d9b5f49611afbfc1489`。利用者がr1を`a0927b26e`でcommitした。
r1の独立設計レビューはREVISE (P1なし、P2 3件、P3 1件)。r2はその全指摘を反映した設計案。
本改訂では製品コードの変更・commit・製品ビルド・mIV起動を行っていない。
r2の独立再レビュー・実装・検収は未完了。§6.1/6.2、§7.4、§8、§10.1を改訂した。

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
`start_merge`のworkerが返すmerge/writeまたはend_merge/save_metasの最終Errを、
Futureへ送る前・そのconsumerの有無に関係なくhookへ1回報告する。
既存のpersist最大4回を使い切ったerrorと、persist以外の書込errorを含める。
merge候補が開始前に無効になっただけの`segment_manager.start_merge`失敗は書込失敗として扱わない。
同期commit/save_metasはdispatcherから報告し、このhookと二重報告しない。

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
短いbacklog起案文を`target/I-1367-design-r2-msg.txt`に載せる。番号付けとリリース優先度は
管理担当の別backlog採録時に決める。本件のFTS修正の受入条件へ名前索引の全failure対応を混ぜない。

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

`metadata_reconfiguration::startup_cleanup` (`src/metadata_reconfiguration.rs:273`) の
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

### 7.4 起動点検の明示的な採用条件 (未達なら見送り)

**A/B計測で起動を遅くしないと判断できることが、点検を製品へ採用する条件。**
既存workerを使うというコード確認やFailed=0のmicrobenchmarkだけでは採用しない。
比較Aは点検なし、Bは点検あり。両者で§5/§6/§8の修正・build条件・fixtureを同じにし、
点検の差だけを測る。以下を全て含む:

- cold/warm、skip_offline_change_scan ON/OFF。
- 84万行以上のコピーfixtureでFailed=0、少数、大量 (Failedが大半) の各状態。
  正常な両ストアの対応を作り、点検による異常再構築の費用と正常起動の追加費用を混同しない。
- idleとI/O競合。I/O競合は使い捨てfixture側への再現可能な背景read/write負荷で作り、
  cache状態・負荷条件・観測の順番を記録する。通常profileのDBを試験に使わない。
- IndexerInit/result-send、overlay解除/UI-ready、Ctrl+G準備完了、metadata supervisor開始の
  経過時間と分布。worker間のCPU/I/O競合による遅延も対象にする。
  `integrity_check_ms`、owner_rows/not_ok_rows、segments/live_docsも記録する。

Rustのコピーfixture計測を先に行う。live起動A/Bは隔離portableの承認済みsuite、または
利用者の手動実施の証拠を使う。複数回・順序を交互にした比較でばらつきと測定分解能を記録し、
再現性のある起動遅延が無いことを確認する。許容遅延を勝手に何msと設定しない。
**いずれかの条件で悪化した場合、または計測未実施/ばらつきで判断できない場合は、
このリリースで起動点検を採用しない。** §5/§6/§8の修正と文書訂正は点検なしでも進める。
起動barrier、全件照会、新index/migrationを足して無理に残さない。
本r2は計画の改訂であり、A/B実施・起動無影響の証明・点検の採用を主張しない。

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
実装前の独立レビュー対象。r2ではr1のREVISEを受けてこれらの設計を改訂し、再レビューへ渡す。

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
   **§7.4のcold/warm・skip ON/OFF・Failed大量・I/O競合を含むA/Bを点検の採用gateとする。**
   未計測/判断不能/悪化なら点検をこのリリースから落とす。

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
本r2は設計のみのため製品test/build gateの成功や利用者確認用binaryを成果として主張しない。

liveの承認をこの調査から推定しない。実装後の大規模再取込/起動A/Bは、別途具体的なscenario・時間・
desktop入力・使い捨てportable範囲を提示して承認を得るか、利用者自身の手動確認へ渡す。
normal profileの実行は利用者だけ。既存事故copyはread-only、通常settingsは実験に使わない。

## 11. 利用者の判断が必要な項目

**現時点の追加質問は無し。** 次の版への採用・起動時間を延ばさない条件は決定済み。
まれな書込failureの通知+次回再構築、retryをatomic操作に限定する技術判断、
点検のSQL/worker配置について利用者へ稀なfailure別の選択質問を作らない。
検証で正常起動が遅くなれば決定済み条件に従い点検を見送る。
通常の機能制限やuser-created dataの破棄が必要と判明した場合だけ、該当項目ごとに
推奨案・失われる挙動/データ・費用を示して別途判断を求める。
