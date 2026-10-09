# §1.356 通常ログ・perfログの非同期writer 設計案

2026-10-09、ライン C。**設計承認済み。2026-10-09の利用者回答を記録し、ラインCで実装する。過去の欠落は警告付きZIP保存、性能gateは判定不能。**
基準: master `70cf4de4e`。本文のコード行番号はこのworktreeの当該HEADで確認した値。
正本: [バックログ §1.356](next-release-backlog.md)、関連: [UI応答性 §4](ui-responsiveness.md#4-新機能追加時のチェックリスト)、[非同期構成 §7](async-architecture.md#7-パフォーマンス計装-perfrs)、[起動診断](startup-diagnostics-plan.md)、[idle health](idle-health-check.md)。

## 1. 観測、原因、保証範囲

- **利用者の観測**: 2026-10-08夜、重いbuild/testを複数worktreeで並行実行中、next-inputのdev buildを利用者が起動し、しばらく「応答なし」になった。調査担当は製品を起動していない。
- 同セッションのperfログから、UI t64568の `app.rs:8728` → `perf::event` のwrite区間が **16,072.7181ms**（4.811→20.884秒）、mutex待ちは0ms。通常ログのVST reader t149460のwriteが **14,243.4417ms**、EffeTune reader t105796の通常log mutex待ちが **14,213.6921ms**。調査時の `perf_events.1.jsonl:839–841`。次frameのupdate **16,079.8882ms**＋App外 **32.3525ms** がframe gap **16,112.1892ms**と一致（同:844–845）。`panic.log`の23:50:16/26、last_ms=4547とも整合。
- reader callbackが通常logへ同期writeするためstderr排出まで止まった。t105796の6.671秒stampはmutex取得前に採取された時刻。t149460のheartbeat群は停止後に排出され、20.885秒の受信stampが並ぶ。pipe満杯によるhost側stderrの追加停止は構造上あり得るが、今回発生したとの証拠はない。
- **OS側でwriteが長時間止まった理由は未確定**。並行build/test負荷は観測条件であり、原因確定ではない。MetricAB loadの終了log時刻とUI復帰の一致だけでは、plugin loadが直接UIを待たせたとは言えない。
- `git diff v4.4.0 HEAD -- src/logger.rs src/perf.rs src/perf/stall.rs` は差分なし。タグ側 `src/app.rs:8684/:88945` に同じperf発行経路がある。v4.4.0にも同じ同期停止リスクがあるとの**コード比較**であり、同版の実機再現ではない。1.337のhost変更はchecked GUI表示許可・非activate表示で、plugin loadの変更ではない。

不変条件: 通常運転のUI／bridge reader／workerは、ログのディスクI/Oやwriterの進捗を待たない。ファイルI/Oの所有者を一つに移す。plugin処理、IPC応答、GUI表示、repaint間隔を変えて症状を回避しない。
本件は設定・画像・catalogの保存workerを変更しない。OS／GPU／pluginの停止そのものやStore審査の起動問題全体を解消する保証ではない。

## 2. 確認した入口・所有・終了境界

| 経路 | 現行コードの事実（file:line） | 設計での扱い |
| --- | --- | --- |
| 通常ログ | `src/logger.rs:11/:117` のprocess-global FILEとinit。mkdir／前回分copy／openを同期実行。`:242/:248/:254/:258` でmutexを保持してrotation／write／flush | facadeを維持し、ファイル所有を専用writerへ |
| perf | `src/perf.rs:37/:56` のFILEとinit_with_path。mkdir／rotation／open、`:262/:270` のmutex＋write、`:287/:301` の明示flush | 同じwriterが別sinkとして所有。perf無効時は従来どおり即return |
| UIの通常記録 | `src/app.rs:87308` のheartbeat。perf有効時の `:87578/:87619/:87637`、通常描画末尾 `:89789` → `:8728` | 発生時刻・tidを採取して非blocking受付だけ |
| startup VST | `src/app.rs:27637` のload worker、`:27761` のUI try_recv、`:27773` の200ms repaint | load／poll設計は維持。ログのための待ちだけ除去 |
| VST／EffeTune reader | `src/video/dsp/bridge.rs:618–625` のstderr pump → `src/video/dsp/mod.rs:1146` の通常logger callback | callbackがwriter I/Oを待たずstderrを読み続ける |
| 現行slow_io診断 | `src/perf/stall.rs:158–166/:238–263` は独立有界メモリqueueへ測定結果を入れ、perfファイルguard下で排出 | writer内のwrite／flush／rotation測定へ移す。producerが旧FILE mutexを待つ構造は撤去 |
| 通常終了 | `src/lib.rs:1854–1873` がRemote／collectionを終了させた後perf::flush。`:1883`／`src/main.rs:5` が起動診断terminal | runtime終了後に一度だけlogging shutdownを完了させる |
| tray | `src/tray_integration.rs:118–123` のQuitは通常close／on_exit経路。hideとquitは別 | hideはwriterを閉じない。Quitだけ通常終了境界へ |
| 早期exit | `src/lib.rs:1002–1008` のexit_startup、`:1904/:1918/:1938/:1948/:1955` のTRT smoke exit、`src/ai/trt_worker_runtime.rs:63/:78/:162` のexit | 初期化済みloggerのbounded shutdownをexit直前へ。未初期化はno-op |
| 診断ZIP | `src/ui_dialogs/preferences/pages.rs:10655–10659` のUI handler → `src/diagnostics.rs:22–36` のexport_requested＋perf／通常flush → `:53` のログ収集 | ZIP処理全体をworkerへ。両sinkの完了barrier＋欠落検査を通してから収集。要求だけのflushに置換しない（§3.5） |
| idle-health／解析 | `scripts/check-idle-health.ps1:409–425` は測定直後に解析し、command line証明で空区間を許可。`scripts/analyze_perf.py:2073–2088` は旧session／frameがあれば空区間をsleep扱いできる | 測定終了境界のdrain checkpointと欠落証明を必須にし、不明なら「判定不能」（§3.6） |

初期化は通常 `src/lib.rs:1228` → `:1256` の順だが、facadeの単独使用／遅いperf有効化（`:1399–1404`）もある。どちらから開始してもprocess内にwriterは一つだけ。`logger::init_for_worker` の別ファイル／append契約（`src/logger.rs:111`）を保持する。

### 2.1 最初の「起動中…」とStore関連

`src/ui_startup.rs:5–22` のrenderer自体はmemory-only。`src/app.rs:87681` がoverlayを描く**前**に、通常heartbeat（`:87308`）、perf有効時のframe.begin（`:87578`）、最初のupdateのstartup.first_frame（`:87619`）、初回から実行するperf::flush（`:87632–87637`）に到達する。creatorの `src/lib.rs:1681/:1704/:1714/:1759/:1845` → `:429` もperf有効時は同期writeである。通常loggerは常時ON（`:1222–1228`）、perfはCLIまたは設定で有効になる。
従って、現行コードには初回overlayの描画前に同期ログI/Oで止まる経路がある。本変更はその経路のファイルI/Oを外す。旧perfのfirst_frameはupdate入口の印であり、実PRESENTを意味しない。常時起動診断のGPU／Present観測（`src/lib.rs:1482–1509`）は別経路として維持する。Storeの観測ログがないため、今回の16秒停止とStoreの報告を同一原因と扱わず、審査問題の修正完了とも書かない。

## 3. 単一writerと非blocking受付（2026-10-09決定仕様）

### 3.1 所有と初期化

process内に一つの `LogWriter` を置く。通常sinkと任意のperf sinkの **mkdir、open、write、flush、close、copy、rotation、retention** はこのthreadだけが行う。facadeは時刻基準・出力先などのmemory設定を渡すだけで、UI上の初期化でもファイル存在確認・open・writer ready待ちはしない。
writer開始前／sink準備中にも有界queueへ受付可能とする。状態は単一の `NotStarted / Running / Closing / Stopped / Failed` 所有にまとめ、facade別のready／pending／stopped boolを増やさない。Runningは受付可能を意味し、ファイルopen完了を意味しない。通常／perfのsink状態はwriter内だけに保持する。perf後発有効化・override pathも一度だけ同じownerへ設定し、複数writer／重複truncateを許さない。
perfのsession.startは、producerがpid／process_start_unix_ms／wall clock anchor／t／tidを採取して初期化設定に保持する。通常queueの満杯で捨てず、writerはperf sink準備後の**最初の行**として書く。その後に当該sessionのperf recordsを出す。idle-healthの同session証明を遅い書込み時刻で作り直さない。

### 3.2 queue、順序、欠落

決定容量は通常＋perf合計 **262,144件、payload合計128MiB、1レコード1MiBまで**（利用者 2026-10-09）。利用者PCでperf平均約470bytes・最大約3,000件/秒を測定し、8,192件ではdisk stall約3秒で欠落するため増量。制限はすべて同時に適用。制限値は内部定数とし、設定画面／DBキーを増やさない。
producerは現在のメッセージ／JSON構築を維持し、enqueue前に自身のelapsed timestamp、OS thread ID、call siteを採取する。writer時刻／writer tidへ置き換えない。queueは既存依存の`crossbeam_channel::bounded`で先に確保する有界journalとし、producerは一度の`try_send`、writerは最大256件の`try_recv`で受け渡す。producerとwriterが共有するMutexを置かず、通常の受付競合をdrop理由にしない。ライブラリ内のatomic CAS調停は使うが、拒否したレコードの再投入、sleep、I/O待ち、通常loggerへの同期fallbackは追加しない。厳密なwait-freeとは呼ばない。
128MiBのpayload会計はqueueから取り出した未処理batchも含み、writeまたは破棄が終わるまで解放しない。固定journalの管理領域、各sinkの固定buffer、session初期化設定は別の固定上限を持つ。sink I/O中はmemory journalもbyte会計のguardも保持しない。
満杯、byte上限、oversize時だけ**新しいレコードを捨てる**。通常のmemory競合による欠落を許した旧案は、実機確認後の利用者指示（2026-10-09）で撤回した。`journal_contended`は旧ログ／解析互換の理由キーとしてゼロで残す。既存queueの古いレコードを追い出さず、採用済みレコードは受付FIFO、同一threadの順序を維持する。異なるthreadの時刻順は保証しない（既存mutexでも時刻採取と取得順は一致しない）。oversizeはメッセージを途中で切ってJSON／既存書式を壊さず、件数に含める。producerの一時的なformat領域はqueue容量とは別であり、無制限入力の新しい収集を足さない。
sink別・理由別のAtomic dropped counterとsession累計first_t／last_tを持ち、writerが復帰したとき／flush／shutdown時に差分と累計・累計欠落の発生時間幅を**queueを経由せず**直接記録する。通常は既存prefix付き `[async-log] dropped ...`、perf有効時は `cat=log, kind=records_dropped`（count／total／sink／reason／first_t／last_t）を追加。既存 `diagnostic_dropped` と別kindにして意味を変更しない。writer内部の自己ログはpublic facadeを呼ばず、再帰しない。ディスクが永久停止／processが強制終了した場合、そのcounterまでディスクへ残せるとは保証しない。

### 3.3 flushとslow_io

`perf::flush()` と `logger::flush()`（現行 `src/logger.rs:290–312`）は **非blockingのflush要求**へ変更する。UIの1秒周期、初期化直後のflushを含め、呼出しが戻ってもwrite完了を意味しない。要求はqueue満杯で失われないcoalesced controlとして記録し、受付済みレコードのwatermarkまで排出後にwriterがflushする。後続の記録が絶えなくてもwatermarkでflushできるよう、無限のqueue空待ちにしない。**保存完了を前提にする呼出元を棚卸しし、診断ZIP・解析gate・終了・testは§3.4の完了barrierへ移す。** 描画経路から完了待ちを呼ばない。
writerが採用したwatermarkはそのflushが終わるまで固定し、追加要求は次のflushにまとめる。初期化のLogger／perf stageは受付とwriter開始要求までの時間になり、open完了と誤表示しない。実際のsink open開始／完了はwriter側診断として別に記録する。
**通常sinkは到着した各batchを直ちにwrite・flushする**（利用者 2026-10-09、C356-4変更）。healthy diskでは従来の毎行flushと同じくcrash直前までの保存を保つ。250ms待ってbatch化しない。**perf sinkだけ250ms周期**（dirty時）と明示flush／shutdownでflushする。アイドル時の新しいUI repaintやログ記録を作らない。周期待ちはwriter側の通知付き待機であり、producer待機ではない。flushはOSへのwriteであってsync_allによる電源断耐性の保証ではない。
既存slow_ioのkind／既存fieldは保持するが、今後はwriter上のI/Oとしてtid／holderを記録する。producerのcaller情報は追加fieldで伝え、UIが16秒待ったかのような旧caller tidの偽装はしない。queue residenceは別kind `queue_delay` とし、batch・sinkごとの遅延件数と最大値だけを集約し、既存write_msへ足さない。診断はslow-onlyかつ有界、1レコードごとの自己診断を増やさない。既存解析の区間推定がwriter I/OとUI停止を混同しないよう `scripts/analyze_perf.py` のhitchesも同時確認する。

### 3.4 保存完了barrierとcheckpoint（r2追加、P2-1／P2-2共通）

要求だけのflushと、**特定のcutoffまで保存できたことを返すbarrier**を分ける。barrierは一つのwriter ownerが扱う固定容量のcontrolで、一般レコードqueueのdrop対象にしない。shutdown／ZIP／測定は同じ完了契約を使う。shutdown以外の要求は一度に一件、競合・control受付不可なら即Busy／失敗を返す。Closingへの遷移は通常barrierより優先し、未完了の通常要求へClosingを返して終了drainへ進む。無制限の保留、retry、優先度queue、固定sleepでの代用は作らない。
producerはfacade入口で、timestamp採取より先にmemory-onlyのpublication ticketを登録し、受付またはdrop確定まで同じ所有を保持する。barrierはその境界のticket cutoffと、cutoff取得**前**のmonotonic coverage時刻を採る。writerはcutoff以前の発行中ticketが確定し、採用レコード（取り出し済みbatchを含む）が書かれ、drop集計まで保存されてからACKする。「queueが一度空になった」「後発イベントのtが大きい」だけではACKしない。遅いproducerが古いtimestampを後からenqueueする穴を閉じる。登録・cohort選択・cutoff・closeは一つのAtomicU64所有にまとめ、producerはI/Oもbarrierも待たない。2つの31-bit active count、cohort bit、closed bitを同じwordで更新する。barrierは一度だけcohortをflipし、発行は原子的な登録時点で旧／新cohortのどちらかに所属する。epoch再確認による拒否をしない。初期cohortはepoch 1に対応する。旧cohortがsettleするまで（要求timeout後も）control_busyを解放せず、slotを再利用しない。shutdownは同じwordのclosedを立ててからClosingを公開する。登録済み発行はその後も採用し、2 countとrecord creditがゼロになってから終了する。31-bit countは生存facade callの数であり、processのthread／stack資源上限内でoverflowしないことを不変条件としてassertする。保留metadataはqueue上限と生存producer数で有界にし、cutoffの未完了発行はworker側のdeadlineで不成立にできる。
`session.start` に追加field `log_delivery_schema=1` とprocess内session IDを付け、writerは `cat=log, kind=drain_checkpoint` をqueue外から出す。checkpointの項目はschema、session ID／pid／process_start_unix_ms、request ID（nonce）、purpose、coverage_t、ticket cutoff／settled・written watermark、normal／perf sink状態、session累計のnormal／perf／診断drop数。**checkpoint自体もflushしてからACK**する。完了ACKは同じ項目を返し、writer受付の時点では返さない。通常／perfの両方を要求したbarrierは片方だけ成功しても完了成功にしない。perf OFFは明示Disabledで、ZIPの通常log barrierは成立できるがperf測定gateは成立しない。
ACKにはfile identityとcheckpoint末尾の確定byte offsetも含める。live analyzerは同じfileのそのprefixだけを読んでcheckpointを照合し、後続write途中のJSONや次sessionのファイルを混ぜない。後続appendを待ってから全ファイルを読み直す方式は採らない。offset／file照合ができない場合も判定不能にする。
dropを時間区間へ細かく再配分する履歴は追加せず、**該当sessionの累計欠落が一件でもあれば保守的に証拠不完全**とする。これはbarrierの保存完了とは別の判定であり、ACKは保存できたprefixと欠落counterを返す。ZIPは今回の要求印が保存できていれば警告付きで保存し（§3.5）、性能gateは過去の欠落でも「判定不能」にする（§3.6）。summaryの受信時刻を欠落発生時刻と誤解しない。診断dropもcheckpointに含め、hitchesの原因照合には不完全の印を付ける。timeout／Busy／writer failure／sink error／不一致／checkpoint未保存は単純な不成立。非shutdown barrierの待機も要求受付から最大2秒、再要求しない。

### 3.5 診断ZIPのworker化と完了契約（r2追加、P2-1）

現行 `src/diagnostics.rs:22–36` はexport_requestedを打ち、両flushを終えてから `:53` の収集へ進む。`:197–232` のtestは独立BufWriterを直接flushするfixtureなので、新facadeが要求だけになる退行を検出しない。指摘を採用する。
UI handlerは要求の開始／進捗／結果のpollだけを行い、DesktopのShell解決、stat、ログ読取り、圧縮、ZIP write／closeとbarrier待機はすべて診断export workerへ移す。App側に一つの `Idle / Running(request, receiver) / Finished(result)` ownerを置き、PreferencesStateの結果だけに別pending boolを足さない。Running中の重複ボタンは無効、通常の閲覧は継続。「保存中…」を表示し、保存完了後だけ既存のpath／「保存先フォルダを開く」を出す。worker開始失敗／disconnectは既存の失敗表示へ渡す。
workerはexport_requestedにrequest IDを付け、通常logにも要求印を入れて、同じ要求を含む両sink barrierを取得する。**今回の要求印のticketがdropした場合は通知して作成失敗とするが、session累計normal／perf dropが非zeroという理由だけでは失敗にしない。** 今回の要求印・採用済みtail・checkpointを保存できたら、過去の欠落があっても残ったログとpanic.log等をZIPへ収集する。要求印以外の欠落は時期を問わず証拠不完全として知らせ、次回起動までZIP機能を禁止しない。perf OFFの場合だけperf witness不要と明記する。barrier timeout等の未完了時は**ログ読取り・ZIP作成に進まず**「ログの保存完了を確認できないため、書き出しに失敗しました」と通知し、自動retryしない。必須snapshotの読取り失敗も作成失敗を維持する。通常のZIP入出力・worker開始の失敗は従来どおり失敗とし、欠落counterだけでこれらの失敗状態を作らない。
barrier完了後のrotationと読取りの競合を避けるため、writerがflush後・後続write／rotation前に対象normal／perfの読取専用handleと確定byte長を取得し、immutableなprefix snapshotとcheckpointをworkerへ返す。workerはその長さまでだけ読む。ファイルownerはwriterのまま、workerに渡すのはread handleだけで、producerやwriterをZIP圧縮終了まで待たせない。active perfの実pathをownerから得て、旧世代・stale sessionのperfを代用しない。保存中にappend／renameされても要求印とcutoff以前のtailを含む。snapshot対象の必須ログ読取り失敗を現行のbest-effort skipで成功扱いしない。他の既存補助ログの収集範囲は維持し、panic／startupの未終了thread全体の停止・drainまで保証するZIPにはしない。
ZIPにはcheckpointとsnapshot byte長を示すmanifestを入れる。manifestはnormal／perf／診断dropの累計・理由別件数と、そこから導出する `evidence_status=complete/incomplete`、不完全理由も記録する。非zeroなら画面で **「保存しました: <path>」に加えて「証拠不完全: 一部のログが欠けています。残っているログを保存しました」** を表示し、保存先フォルダを開く操作も維持する。結果は `Saved(path, checkpointの完全性情報)` として一つのexport ownerが保持し、失敗結果に変換したり、別の警告pending boolを作ったりしない。ZIPのfinishと出力buffer flushまで成功してから結果を返し、未完成ZIPを成功表示しない。partialはworker内でbest-effort削除し、再回復処理を作らない。設定画面を閉じてもApp ownerが結果をpollできる。App終了はexportを待ってUIを止めず、requestを退役し、workerは公開前の退役検査で未完成成果物を成功通知しない。worker完了前の強制終了ではpartialが残り得る。新しい他操作禁止／終了禁止のモーダルは導入しない。
根拠再確認（r3）: `src/diagnostics.rs:13` はbest-effortと明記し、`:87–96` の補助ログ読取り／ZIP entry失敗はskipして残りを収集する。残った診断情報を渡す既存機能を保つため、累計dropで以後のZIP作成を禁止するr2案を撤回する。**ZIPが保存できたことは、ログが完全で性能gateを通せることを意味しない。** manifestがincompleteなら解析側は§3.6どおり判定不能を維持する。

### 3.6 perf消費側の完全性契約（r2追加、P2-2）

現行 `check-idle-health.ps1:409–425` は測定終了後すぐanalyzerを呼び、command lineで対象を確認できると `--allow-sleeping-window` を付ける。`analyze_perf.py:2073–2088` は空区間でも過去の同session／frame証跡からsleepとして扱い、`:2381–2385` のhitchesはdiagnostic_droppedだけを表示する。新writerの未保存tailやrecords_droppedをこのまま判定すると偽PASSになるため、指摘を採用する。
**live idle gate:** 測定終了時のCPU／foreground／window／時刻をまず採取して区間を固定し、測定後に専用のlocal logging-control named pipeへnonce付きbarrier要求を一度送る。UI handler／Remote操作権／App update／repaintを経由せず、専用control workerが一般writerのcontrolへ渡す。通常運転中にcoreを閉じてflushさせる方式は採らない。pipeはcurrent user限定DACL＋remote client拒否（既存の根拠: `src/remote_ipc/pipe.rs:4032–4050`）、pid／creation time／session IDを照合し、読めるのは完了情報だけ。任意pathや設定操作は受け付けず、1件の有界request／responseと最大2秒deadlineで処理する。既存Remote wire schemaへ混ぜない。
scriptはACKとファイル内の同nonce checkpointの一致を検査し、session clock anchorを使った測定end_tをcoverage_tが覆い、watermark完了、必要sink正常、drop数既知かつzeroであることを確認してから解析する。ファイルの最大tやサイズ増加、250ms経過、session.start／過去frameだけを代用しない。末尾が遅延中、nonce／session違い、counter不明、欠落、timeout／Busy／closed process／非対応buildなら **CannotJudge（判定不能）、exit 2**。性能違反FAIL（exit 1）と区別し、gateは両方とも通さない。正常な空区間は、この完全性証明と外部process証明がそろった場合に限りsleepとして評価できる。排出を待った時間のCPUは測定区間へ混ぜない。ログ増加量は開始・終了barrierのbyte watermark差を使い、測定外tailの遅いflushを静止中の増加と混同しない（開始barrierも測定開始前に実行し、CPU採取はその後）。
**analyze_perf共通:** idle-health、hitches、scroll、startupは共通の完全性validatorを通す。明示した区間末尾を覆うcheckpointとそのsession・nonceを指定するか、封じたshutdown／診断ZIP manifestのcheckpointまでを対象にする。引数未指定の解析で単に最後のevent tを「全測定の終了」にしない。新ログで未確認tailがある場合と、完全性証明を持たない旧ログは、既存値を「参考値・証拠不完全」と表示できるが、区間の完全性やヒッチなし／scroll完了／startup到達を断定せず、検証用途はexit 2にする。旧書式の読取りは維持し、旧ログの解析値を破壊しない。
hitchesはrecords_dropped（sink／理由／count／total）とdiagnostic_droppedの両方を表示し、欠落／coverage不足を先頭で明示する。欠落を「ヒッチ0件」「待ち先なし」の根拠にしない。scrollはsettle→first/all readyの未完了を未保存tailと区別できなければ未判定、startupはfirst_frame／目的の終点が欠落またはcoverage外なら未判定。perf以外の通常log欠落もcross-log照合を不完全にする。counterのdeltaとtotalを足して二重計上しない。ログにないdrop summaryをzeroと推定せず、checkpointの明示counterを用いる。

## 4. 独立して残す診断と子プロセス

「失ってはいけない」は通常queueのdrop対象へ移さないという契約。ディスク故障、強制kill、native crashでも必ず保存できるという保証ではない。

| 診断 | 現行根拠 | 決定案と理由 |
| --- | --- | --- |
| Rust panic／panic.log | `src/lib.rs:504–524` は通常logger→panic.logの順。`:532–551` は独立open／append、4MiB＋.bak | **panic.logへ先に直接同期記録**し、その後通常ログへbest-effort enqueue。一般queueをdrain／joinせず、writer mutexに触れない。unwind中やqueue満杯でも主証跡を通常loggerへ依存させない。panic hookからshutdownを実行しない（worker panicでprocessが続く場合もある） |
| native exception | `src/lib.rs:865–886` は既存例外情報をpanic.logへ記録 | 独立経路を維持。新しいalloc／lock／writer待ちを足さない。既存native handlerの限界は本件で拡張しない |
| UI hang watchdog | `src/lib.rs:571–578/:634–638` は専用threadからpanic.logへ直接記録 | panic.log直接同期を維持。通常ログ複写はenqueueだけ。native-healthもpanic.logを先に書く。UIや一般writer停止中でも別の証跡を試みられる |
| 常時startup JSONL | `crates/startup-diagnostics/src/lib.rs:212–231` の非blocking journal、`output.rs:32–40/:105–140` の独立writer、`lib.rs:654–697` のterminal | **別owner／writerのまま**。一般writerへ統合せず、通常／perf queueが停止・満杯でもstage／watchdog／terminalを公開できる。既存容量・drop／retention／独立watchdog契約を保持。終了時のterminal排出ACKは§5の共有境界で補完する |
| Remote service log | `src/remote_ipc/service.rs:440–448` はRemoteへ--logを渡し、別processが `crates/remote-web/src/diagnostics.rs:16/:150–161` でrequest JSONLを同期保存 | **Remote側の専用ログは現状の同期のまま**。本体UIのwriterとは別process／別ファイルで、秘匿値除去・request schema・retentionを維持。Remote自身のI/O応答性改善は本件に混ぜない。core側stderr monitor（`service.rs:496–503`）の通常logは今回の非blocking facadeで改善 |
| PDF child | `src/lib.rs:1093–1101` はlogger初期化前にworker分岐。`src/pdf_loader.rs:2949/:2913–2929` はstderr pipeとreader→通常logger | childの同期stderr出力／IPC framingを維持し、parent readerだけログI/Oを待たなくする。childがmain logを初期化／truncateしない契約を保持 |
| Susie child | `src/susie_loader.rs:1072` のstderr=null、`crates/susie-worker/src/main.rs:34–38` のstdin／stdout | 新しいstderr収集／logファイルを加えない。parentの通常診断のみ非同期化。stdoutはbinary IPCで、log queueへ流さない |
| VST3／EffeTune host | `bridge.rs:618–625` のreader。`crates/vst3-host/src/main.cpp:1301–1302/:1323–1327` のstderr printf／fflush | C++ stderr／protocolは維持。parent排出を非blockingにする。child stdoutの制御応答をログdropで失わせない |
| TRTなどlogger使用child | `src/ai/trt_worker_runtime.rs:31` はinit_for_worker、`:63/:78/:162` は直接exit | facade共有なのでprocessごとのwriterを使用し、専用append先を維持。直接exit前にbounded shutdown。pdf／Susieと同じ初期化と決めつけない |
| EPUB／その他reader | `src/epub_convert.rs:569–572` のstderr→logger | 同じfacadeを使うreader／workerを横断監査。IPC結果／進捗のchannelは変更せず、stderrのdiagnostic複写のみ対象 |

### 4.1 allocラインの .lock sidecar と取り込み境界

利用者指定のstartup JSONL **`.lock` sidecar修正を維持する**。ただし基準HEADでは `crates/startup-diagnostics/src/output.rs:61` がJSONL本体をshared lockし、`:228` のretentionも本体をexclusive lockするコードであり、sidecar修正はまだこのHEADに無い。これは取り込み順の依存である。利用者指定commit `90a8a9353` のsidecar修正／testsだけを現行session_headerへ移植し、同じsidecarをwriterの生存証明／retention判定に使うことを確認してから実装・検証を完了する。他worktreeを編集しない。本件でJSONL本体lockへ戻す変更や独自の二つ目のlock方式を作らない。sidecarの既存schema／寿命／cleanupはalloc側仕様を正本とする。

## 5. 終了、crash、まれなI/O失敗

1. 通常close／tray Quitは既存のsettings・sidecar保存、App／Remote／collection／child終了を先に行う。App::on_exit内で一般writerを止めず、終了workerの最後のログを受付できるままにする。tray hideでは何もしない。
2. run_nativeとruntime cleanupの後、起動診断へ実際のterminal outcomeを公開する。通常logのshutdown開始印を受付後、一般ownerをClosingへ一度だけ遷移。producer登録とcloseを同じatomic ownerで確定し、writerが取り出し中のbatchとqueueを含め、close前に登録済みの発行を排出する。遷移時にcritical section内だったproducerとdrainの順序をmemory境界で確定する。Closing後の通常レコードは採用しない。panic.log／startup terminalはこの受付制限の外側。
3. workerはdrop集計→両sink flush→close→完了通知。shutdown呼出元は**最大2秒、一般writerとstartup writerを合わせて一つのdeadline**で完了を待つ。待機をwriter数だけ積み重ねない。成功なら完了済みthreadをjoinできる。timeout／writer failureならjoinやFile／BufWriterのdropを呼出元で行わず、threadとI/O所有を残してprocess終了へ進む。再flush／同期fallback／retryはしない。これは終了時間の上限であり、UI停止をtimeoutでごまかす修正ではない。
4. `src/main.rs` の通常return、`exit_startup`、scripted exit、TRT worker／smokeの直接exitを同じidempotent終了helperへ接続。writer未起動は即return。終了後も生きるwatchdog等は一般writerを再作成しない。loggerを使う開発binのreturn／exitも棚卸しし、独立binaryのログtailを失わせない。
5. panicでは先に直接panic.logを残す。unwind後に通常returnへ到達する場合だけ通常終了helperを通す。abort／native fault／TerminateProcess／電源断はdrainを保証しない。panic hookで全queueを直接書く方式は、停止writerと同じdiskを待ち、二重所有／再入deadlockを増やすので採らない。

startup writerのterminalは現行 `lib.rs:654–697` がpublish＋wakeだけで、flush完了ACKはない。また `output.rs:105–145` はbatch処理後にStoppedを見て戻るため、64件を超えるpending全体の排出保証でもない。**必要な最小共有変更**として、独立ownerが採用済みterminalまでのpendingを排出し、最後のflush／close後にACKする終了契約をstartup／alloc担当と合意する。容量上限到達／sink failureは成功ACKと区別する完了結果だけを返し、再試行や新しいqueueは足さない。通常publish／watchdogはACKを待たず、core／launcherの最終終了境界だけが同じbounded deadlineで待つ。一般writerがstartupファイルをopen／lock／flushする変更はしない。取り込み時にterminal順序・.lock sidecar寿命・retentionの担当をcoordinatorが確定し、既存terminal testに排出完了testを追加する。先行変更だけでACKがあるとは扱わない。
terminalの最終1件は独立ownerの終了metadataとして保持し、通常journalの満杯／try_lock競合で失わない。writerが既存schemaのterminal行を末尾へ一度だけ出す（従来queue内との二重出力は禁止）。ファイル上限にもその一行分を予約する。途中stage記録の既存drop上限は維持し、通常queue全体を無損失化する仕組みは追加しない。この小さい保護とACKをstartup側の同じ終了ownerに置き、一般loggerに複製しない。
open／write／rotation失敗はaffected sinkをそのprocess中Unavailableにし、自動再open／retry／回復queueを足さない。他sinkとUIを続け、memory statusへ一度だけ失敗を公開し、次の通常UIで「診断ログを保存できません。次回起動時に再試行します」を一度通知する。次の起動で通常初期化を試す。失敗通知を失敗sinkへ書く再帰を作らない。shutdown timeout時は新しい同期panic.log書込みも試さない（上限を破るため）。未flush tailの欠落はC356-3／4／5の2026-10-09回答で承認済み。

## 6. 状態の簡素化と互換性

- ログ記録中をモーダルにする案は不採用。観測処理が閲覧／IPCを禁止する機能変更になり、disk stallをUIへ戻す。一つの一般writer、固定上限、drop-new、終了時の一回だけのsealにより組合せを減らす。
- per-thread writer、sinkごとの一般writer、無制限queue、優先度別queue、古い記録の退避、再試行／disk spool、panicから一般queueをdrainする案は不採用。通常／perfの片方が止まると両方の保存は遅れるが、producerが止まらず、重要診断は独立経路なので、一つのownerで足りる。ZIPは一つのworker要求、barrierは一件だけ。累計欠落のmetadataをそのまま使い、ZIPでは警告、性能gateでは判定不能とする。時期別のdrop履歴、欠落の埋め直し、export retry、画面を止めるモーダルを増やさない。
- released-data: 通常／perfログはv4.4.0以前から存在する**リリース済み診断形式**。設定・タグ・collection等の利用者データではないが、解析toolの入力なので既存書式を維持する。DB schema／設定migration／既存データ削除は無い。
- 通常prefix `[elapsed s][tID]`、perfのt／tid／cat／kind／key／seq／extras、session.startのpid／wall anchor、時刻基準、UTF-8、改行、pathを維持。writer受信時刻でstampしない。新kind／delivery schemaは追加だけ。`scripts/analyze_perf.py:62–80` のJSONL readerと、`scripts/check-idle-health.ps1:228–250/:299–319` のpath／session証明にfixtureで照合する。**書式が読めることと、測定が完全なことは別**。§3.6の共通validatorとCannotJudge契約も互換性gateに含め、旧fixtureの参考値を保ちつつ、完全性不明をPASSしない。
- 通常は現行16MiB・1 rotated generation、debug retentionは256MiB・4 generation、起動時.prev保存（`src/logger.rs:13–22/:120–134`）を維持。perfはcurrent＋過去4世代、override pathはrotation無し（`src/perf.rs:42/:56–93`）を維持。新workerが既存ログ以外をretention対象にしない。panicは4MiB＋.bak、startup／Remoteの独立retentionも維持。
- 実装時のdoc co-update: `async-architecture.md` §worker一覧／§7、`ui-responsiveness.md` のログ診断境界、`idle-health-check.md` の完了証明と判定不能、manualの診断ZIPの保存中／証拠不完全での保存／失敗説明。既存ログの保存先と外部通信先は変えず、local logging-controlとZIP manifestについても製品ページ・privacyの説明を整合させる。ZIP進捗／重複禁止／警告付き保存／失敗UIのsnapshotを追加する。

## 7. 検証計画（実装時、実アプリ起動無し）

| 検証 | 採用基準 |
| --- | --- |
| 有効なred／実経路のstall | fake sinkのwrite、flush、open、rotationをそれぞれlatchで止める。logger／perf facadeとfake stderr pumpを通し、sink未解除のままUI役／reader役producerが全件処理を終えることをchannelで検査。現行同期版は解除前に完了できないことをredとして記録。時間sleepでlockの状態だけを見るtestにはしない |
| bound／drop | 件数／bytes／oversizeで非blocking drop。通常のjournal競合では全件採用し、上限不変、採用済みFIFO、回復後のdrop差分／累計、perf OFF時も通常summary、summaryが満杯queueに再投入されないことを固定 |
| timestamp／ordering | 複数threadから既知stamp／tid／連番を投入。各threadの採用済み順序を維持、writer tidで置換無し、同threadの正常系を無欠落で検証。別threadのwall-time sortは要求しない |
| init／session／flush | 通常のみ／perfのみ／後発perf／override／重複initでwriter一つ。open中にも受付可能。session.startが先頭かつ満杯で消えない。UI flushがfull queueでも戻り、watermark前をflushして後続無限流入で飢餓にならない |
| barrier／publication | timestamp前に登録したproducerをlatchで途中停止し、queueが空でもcutoffのACKが出ないことを実APIで検査。確定後だけrecord／dropを含むcheckpointとflush ACKが出る。後発publicationで飢餓にならず、full queueでもcontrolが捨てられない。Busy／Closing／timeout／write失敗は成功ACK無し。通常・perf・checkpoint自身のflushをそれぞれ止めてもUI役は待たない |
| 診断ZIPの実配送 | 独立BufWriterだけの旧testでは代替しない。注入可能な同じLogWriter owner／logger・perf facade→worker barrier→snapshot→ZIP収集を通し、64KiB未満の両tailとexport_requestedがZIPにあることを検査。barrier未完了ではFile::create／ログread／成功表示が0回、解除後だけ成功。容量上限でdropを作ってから復帰させ、今回の要求印／tail／checkpointと残ったログ・panic.logを収集でき、manifestがincomplete＋drop件数、画面がpath付き保存＋証拠不完全になることを固定。同じfixtureの性能gateはCannotJudge／exit 2。perf OFF／今回の要求印drop／timeout／必須snapshot読取り失敗／Busy／sink failure／partial cleanup／画面close／App終了／snapshot後rotationも検査。UI handler開始・poll、重複禁止、完全な保存・警告付き保存・失敗のsnapshotを固定 |
| rotation／retention | disposable dirと小さいtest上限で.prev／.bakN／perf世代／overrideを実ファイルで検査。unrelated files・設定等のhash不変、起動中read共有、rotation後も順序／完整JSON行を検査 |
| shutdown | 採用tail／取り出し済みbatch／drop summaryまでflush後ACK。Close／tray Quit／早期exit／scripted／worker exitが同じhelper、hideは停止無し。fake clock／completion channelで最大2秒を検証し、未完了threadにjoinせず、timeout後producerがI/Oへfallbackしない |
| 重要診断 | writer永久stall＋queue fullでpanic／watchdogのfake direct sinkへ先に記録できる。catch_unwind後にprocess状態を壊さず、native経路へ新待ち無し。startup publisher／watchdogは一般writerから独立、terminalはfull／競合／pending>64件／ファイル上限でも予約した最終行を一度だけ排出してACK。startup sink自身のstall時は合計deadlineを超えず、allocの.lock retention testは維持。Remote secret-redaction／request logは維持 |
| 失敗・解析 | sink failure時は一度通知、他sink継続、retry／再帰無し。既存v4.4.0 fixtureと新writer fixtureで既存参考値・JSONL読取りを維持。§3.6の完全性validatorはidle-health／hitches／scroll／startupへ共通適用し、下記headless回帰を必須にする |
| 未保存tailによる偽PASS | 既存session／frame／tail_repaintだけが保存され、測定区間のeventsはwriter sinkのlatchで未保存という実経路を作る。外部sampler証明＋allow-sleeping-windowがあってもcheckpoint無しでCannotJudge／exit 2。sink解除→同nonce checkpoint保存→ACK後にだけ評価する。何秒sleepしたかを成功条件にしない |
| 欠落による偽PASS | records_droppedがある場合、summary未保存でもACK counterがnonzeroの場合、counter不明、diagnostic_dropped、nonce／session違い、coverage不足、file末尾の不完全JSON、旧buildの全ケースで判定不能。headlessの `scripts/test_analyze_perf.py` とscriptのfake control／analyzer経路で非zeroとstatusを固定。完全・drop zero・checkpointありの空区間だけPASS可能 |
| hitches／scroll／startup | records_droppedとdiagnostic_droppedを理由・sink・累計付きで表示し二重計上無し。欠落でhitches=0を断定しない、scrollのall_ready未保存を完了扱いしない、startup終点未保存を到達扱いしない。旧ログは参考値＋証拠不完全、checkpoint付き正常fixtureは従来数値。writer診断のtidとqueue_delayをUI停止へ誤帰属しない |

focusedで有効なred→greenの後、`cargo fmt`／`cargo fmt --check`、`cargo test -p mimageviewer --lib`（pipe無し・実exit）、normal／portable `cargo check -p mimageviewer --bin mimageviewer-core`（portableは `--features portable`）、`python scripts/check_ui_glyphs.py`。startup crateも単体testし、UI失敗通知を変更するならui_snapshot。共用loggerの横断変更なので最終 `scripts/test-full.ps1` はcoordinatorと担当を決め実施し、valid evidenceを共有する。依頼された実装時にはbuild-devを作るが起動しない。実装roundでは指定された全gateとbuild-dev -PreserveRuntimeを実行し、製品は起動しない。
利用者の実機確認は、coordinatorがbuild後に手順を渡す。起動・VST復元中の操作、perf ON/OFF、tray Quit、診断の保存と欠落summaryを確認する。重い並行build/testは観測条件だが、負荷を掛けた実機確認は別途利用者と計画し、こちらでは実行しない。

## 8. 利用者決定（全5項目回答済み、2026-10-09）

1. **C356-1 容量（変更して決定）**: 262,144件・本文合計128MiB・1件1MiB。内部定数とし設定を増やさない。利用者PCの実測を理由として旧推奨8,192件から増量。
2. **C356-2 捨て方（推奨を採用）**: 満杯では新規記録を捨て、保存再開時に欠落件数と発生時間幅を記録。競合による欠落の承認は、実機確認後の追加決定（2026-10-09、§14）で撤回。panic／watchdog／常時起動診断は独立経路を維持。
3. **C356-3 保存完了と終了（推奨を採用、C356-4に合わせ周期を明確化）**: perfは250ms周期。終了は一般writerとstartup writer合計2秒。診断ZIPはworkerで完了barrierを待つ。過去欠落はmanifest／画面で証拠不完全を知らせZIP保存、今回要求印drop／確定snapshot内の要求印消失／barrier timeout／必須snapshot読取り失敗は作成失敗。性能gateは完全性不明・欠落ありなら判定不能。自動再試行なし。
4. **C356-4 crash時（変更して決定）**: panic.log／UI hang watchdogは直接writeを維持。通常sinkは各到着batchを直ちにwrite・flushし、250ms周期にしない。perfだけ250ms周期。異常終了でqueue全体の排出は保証しない。ディスク障害や強制killでpanic.log自体の保存も保証できない。
5. **C356-5 まれな保存失敗（推奨を採用）**: 一度通知し、affected sinkは次回起動までUnavailable、自動再試行なし。他sinkと閲覧を維持。設定・タグ等の利用者データには触れない。

未回答の利用者質問はない。実装には`90a8a9353`のstartup診断.lock lease修正と試験だけを移植し、allocator flavor／session_header変更は持ち込まない。

## 9. r2レビュー指摘への対応記録（2026-10-09）

- **P2-1採用**: `src/diagnostics.rs:22–36` の両flush完了契約とUI handler `preferences/pages.rs:10655–10659` を照合。r2で§3.4／§3.5にworker・完了barrier・要求印／tailの固定prefix・timeout／drop時の通知と失敗を追加。§7に同じfacade／writerを通す回帰を追加し、C356-3を変更。**累計dropをZIP失敗とする部分はr3で警告付き保存へ訂正（§10）**。旧独立BufWriter fixtureだけでは代替しない。
- **P2-2採用**: `check-idle-health.ps1:409–425`、`analyze_perf.py:2073–2088/:2381–2385` を照合。§3.4／§3.6／§6／§7に保存済みcheckpoint・欠落counter・完全性validator・CannotJudge／exit 2を追加。既存sleep許可test（`scripts/test_analyze_perf.py:1182–1186`）と診断欠落test（`:1830–1838`）だけでは新経路を証明できないため、未保存tail／records_droppedを含む実配送・headless回帰を追加する。レビュー担当による偽PASS報告はコード上の分岐と整合するが、この文書改訂ではtestを再実行していない。
- **P3採用**: PDF分岐を `src/lib.rs:1093–1101`、perf FILEを `src/perf.rs:37`、世代定数を `src/perf.rs:42` に訂正。logger初期化前のPDF分岐という前提は維持。r2照合HEAD `d3a125c10` は基準HEADから文書のみの変更で、製品コードの基準は同じ。

## 10. r3レビュー指摘とcoordinator決定（2026-10-09）

- **P2採用、coordinator決定を反映**: `src/diagnostics.rs:13/:87–96` のbest-effort収集と既存UIの保存path表示（`src/ui_dialogs/preferences/pages.rs:10662–10675`）を再照合。過去の欠落だけで以後の診断ZIPを禁止する必要はなく、残ったログ／panic.logを渡す既存機能を維持する。
- §3.4／§3.5／§6／§7／C356-3を統一し、barrier保存完了と証拠完全性を分けた。過去の累計dropはmanifest・画面で証拠不完全を表示してZIPを保存。今回の要求印drop、barrier timeout、必須snapshotの読取り失敗は作成失敗を維持。性能gateは証拠不完全なら判定不能のまま。
- この取扱いはcoordinatorの決定済み仕様。C356-3にはその説明を含め、保存周期／待機上限などの残る利用者回答を求める。製品コード・テスト・アプリ起動は変更／実行していない。

## 11. 実装の所有・境界記録（2026-10-09）

- `src/log_writer.rs` の単一Service／Workerが通常・perf sinkを所有。facadeはメモリ設定・publication登録・format・一回のtry_lock受付のみ。262,144件／128MiB／1MiBの制限はqueueと取出し中batchを合算する。通常batchは即flush、perfはdirty時250ms。定常idleでは新しいUI repaintを作らない。
- Lifecycleは一つのtyped atomic phase（Running / Closing / Stopped / Failed）。ファイル状態はworker内のDisabled / Ready / Failed。別のinitialized boolを持たず、この状態で一回だけ初期化する。openの開始／終了時刻もworkerが採取し、slow-onlyのopen診断として残す。保存失敗の通知はClear / Pending / Reportedという一つのownerで一度だけ渡し、sinkは再openしない。BufWriterの失敗bufferを破棄する際もdropによるwrite再試行を避ける。
- publication ticketは個別recordの履歴を無限に保持せず、単調増加のcheckpoint境界番号と、二つのactive count／cohort／closedをまとめた一つのatomic所有者を使う（実機確認後の修正、§14）。登録はtimestampより前に原子的にcohortを選択・加算し、cutoffと競合しても拒否しない。容量による拒否の欠落counterはactive countを減らす前に更新し、barrierが未集計の拒否を完了扱いしない。barrierは旧cohortが全settleするまでACKせず、timeoutでも旧cohortが終わるまで次のbarrierをBusyとし、slotの再利用を防ぐ。checkpointのticket_cutoff / settled / writtenはこのpublication境界番号であり、record件数ではない。境界以前のjournal prefixを固定件数だけ排出し、後続流入で飢餓にならない。
- coverageはepoch変更前の絶対monotonic Instantを保持し、保存時に当該perf STARTへ換算する。perfの後発有効化がbarrierと交差しても、初期化時刻との差で過大なcoverageを作らない。producer timestamp／tidは既存record内で保持する。
- 復帰した各batchの後で欠落summaryを記録し、queue全体が空になるまで通知を遅らせない。ACK前にdrop集計と両sinkをflushし、最後にcheckpoint自身をflush。checkpointの書込みを自己診断として後ろへ再帰出力しない。checkpointの総数は同時に採取した理由別counterから合計し、後続producerの欠落で両者が食い違わない。通常終了もshutdown checkpointを末尾に残す。file identityと確定offsetはcontrol ACK、固定prefix handle／長さは診断ZIPへ返す。
- `src/log_writer/control_pipe.rs` はcurrent-user DACL、remote拒否、first-instance保護のlocal endpoint。UI／Remote操作queueを使わない。requestは4KiBまで、schema／pid／OS process creation／session／nonceを照合。overlapped transportはdeadline付きで、取消完了までbuffer／OVERLAPPEDを保持する。保存完了を示すACK以外の機能は持たない。
- `src/lib.rs::finish_logs` はprocess内一つの2秒deadlineを保持し、run/mainの再呼出しでも待機上限を延長しない。runtime cleanupと実terminalを先に完了し、一般／startupを同じdeadlineで待つ。早期exit／scripted exit／TRT worker／bench_aiも接続。tray hideは変更しない。
- `90a8a9353`からstartup .lock sidecar／retention試験のみ移植。allocator flavorやsession_header refactorは移植しない。startup writerは独立のまま、terminal保護・全pending排出・最終flush／close後ACKを追加。
- 診断ZIPは実writer→自分の要求印→barrier→snapshot→ZIP経路で検証する。perf gateはcheckpoint・欠落数・session・nonce・coverage・prefixを確認し、旧ログは参考値だけ残してCannotJudgeとする。過去欠落のZIP警告は実配送fixtureとUI snapshotで確認する。
- 同期loggerの有効REDはactual `logger::log`を使う隔離test harnessで取得（`target/C1356-legacy-logger-red.txt`）。通常profile／製品binaryを使わず、旧FILE所有guardを保持するとproducerの完了channelが解除前に届かず失敗した。新facadeは同じ隔離testと実worker I/O latchで検査する。最終gate結果はhandoffに記録する。

## 10. 実装レビュー修正（2026-10-09、642ed1ee5への追補）

6件とも指摘を採用した。まれな失敗を再試行する仕組みは追加しない。

- **P2-1・増加量**: ACKに`normal_written_bytes`／`perf_written_bytes`を追加する。session内で実際にwriterへ書き込んだ累計byte数で、通常logのrotation後も減らない。perfはbufferを排出・flushしてから確定する。session header、通常record、drop summary、slow_ioを含み、自己参照を避けるためdrain_checkpoint自身だけをperf byte数から除外する。gateは開始・終了の成功ACK差で閾値を評価し、取得前のファイルサイズ差は使わない。identity不一致・counter不明・逆行ならCannotJudge。analyzerもcounterの型・非負・ACKと保存checkpointの一致を検査する。
- **P2-2・process終了**: launcherの正常return／エラーexitは同じ最終境界でterminalを公開し、startup writerのflush／close ACKを最大2秒で待ってから終了する。エラー画面は排出待ちの後。TensorRT builderのusage／モデル拒否／成功／全エラーexitはcore共通の終了helperへ接続し、一般writerとstartup writerで一つの2秒deadlineを共有する。unit-test executableの子processで実際の終了経路を通し、pendingと末尾terminalを確認する。製品binaryは起動しない。
- **P2-3・perf初期化**: session headerのwrite／flush失敗時も`BufWriter::into_parts()`で未保存bufferを破棄し、Failedと一度通知へ進む。dropの暗黙write再試行をしない。書込み回数を数えるsinkで初期化失敗の回帰を固定する。
- **P2-4・ZIP印**: enqueue成功に加え、完了barrierで取得した通常／有効perfの固定prefix内に、この要求IDの`export_requested`があることをworker上で検査する。印がrotationで消失した場合は既存の失敗通知でZIP作成を中止する。バックアップ収集・過去dropの警告付き保存を代用としない。snapshot取得前に複数rotationする実配送回帰を追加する。過去のdropだけではZIPを禁止しない。
- **P2-5・flush診断**: 計測したperf flushのIoProbe.finish後に、診断queueをもう一度排出・flushする。この最後の排出は計測しないので自己診断を再帰生成しない。残った診断がある場合はperiodic flush対象を維持し、completion ACKは成功にしない。通常logのIoProbeだけが診断を作り、perf recordが無い場合も診断queueを周期排出の対象にする。実IoProbeがslow_ioを生成する停止fixtureで、追加イベント無しでもshutdown checkpointより前に保存されることを確認する。
- **P3・受付試験（当時の記録。現行は§14で有界channelへ変更）**: test専用fixture_gateを廃止し、producerとwriterの受付競合は本番と同じtry_lockを通す。latchはworkerの各段階・dequeue guard保持中を止めるだけで、受付の同期方式を変更しない。採用された各threadの連番・stamp・tid、拒否数とbarrierの理由別drop集計、guard競合中にもproducerが戻ることを検査する。

利用者の2026-10-09決定と、未回答の質問がない状態は維持する。

## 14. 実機確認後の通常負荷の欠落修正（2026-10-09）

- 利用者はpid 145892、7-plugin構成で目立つ停止なし・起動update最大162msを報告した。これは利用者の観測であり、調査担当は製品を起動していない。`target/c356-hw1`のコピーはperf 1,625行／519,936 bytes、t=56.000までを含み、perfの欠落は60件（t=0.685201〜3.221293）、通常の欠落は17件（t=0.789977〜54.825711）。最初の36秒相当では通常14件。容量不足・oversizeの欠落、drain_checkpointはこのコピーにはない。1秒bucket（tの整数部分）で数えた保存perf行の最大は752行で、コピー全体でも決定容量より小さい。ただし捨てた内容・瞬間的queue長は記録されていないため、ログだけから厳密なqueue peakや欠落レコードの種類は復元できない。
- 根因は旧`Service::enqueue`の`try_lock`。writerの`batch`は空queueでも同じMutexを取り、最大256件のdrain用Vec確保もguard内にあった。wake／次レコードが重なるだけで、容量に空きがあってもdropする。t=10.429／15.234等の通常欠落はsimilar reconcileのstart／phase記録の並びに一致する。heartbeat由来とは断定できない。副作用を隠す容量増加、writerの遅延、producerの再試行は採らない。
- **利用者の追加決定**: 通常負荷の受付競合で通常／perfレコードを捨てない。§3.2を有界channelへ訂正し、262,144件／128MiB／1件1MiBは維持する。record／byte creditはchannel内・発行予約・取り出し済みbatchを含めraw write後まで保持する。この順序により、producerはconsumerが未解放の再利用slotを待たない。channelの両端はServiceが所有し、PublicationがServiceのArcを保持するのでDisconnectedを不変条件違反として扱う。
- 同型の欠落であるbarrierと登録のepoch競合も除去する。§3.4の単一atomic登録所有にまとめ、旧／新cohortを跨ぐ通常発行を拒否しない。checkpoint schema／時刻・tid／sink別counter／既存理由キーを維持する。登録metadataを追加のbool／pendingで分割せず、既存active[2]を単一wordへ置換して状態組合せを減らした。loggerはmodalにすると通常閲覧を妨げるため採らない。
- crossbeamのtry APIにはmemory CAS調停がある。producerにdisk待ち・再投入loopはない。writerのtry_recvは、producerがslot予約後・公開前にpreemptされた場合にその小区間の公開を待ち得る。完了barrier／shutdownの呼出元は従来の合計2秒上限を保ち、writerをjoinして待ち続けない。別のslow-only `perf::stall::DiagnosticQueue`は「preempted producerがwriterを止めない」契約を持つため、同じchannelに機械的に置換しない。そちらの既存128件上限・欠落通知と、startup診断の独立queueは本件では維持する。
- 回帰: 実writerをdequeue段階で止めてもproducerが戻りレコードが保存される、同時producer全件／各thread順序／stamp・tidの保持、8 threadから通常・perf計3,200件のburstが欠落ゼロ、旧cohort登録がcutoffを跨いでも保存・dropゼロ、新cohort保持中の旧barrier完了、timeout後の旧slot再利用禁止、close前登録の保存とclose後拒否。dequeue競合／cutoff競合の2件は旧コードで失敗する有効なREDを取得した。旧memory競合dropを期待する試験は、新しい不変条件を検査する試験へ置き換える。
- 自動検証（2026-10-10）: dequeue／cutoff競合の有効なRED 2件からGREENへ。focused log_writer 31成功／1無視、diagnostics 27、logger 3成功／1無視、stall 10、panic／watchdog 1、全lib 11,450成功／55無視・exit 0。fmt／glyph／normal・portable check、Python 76件／PowerShell 20件も成功。追加test-fullの初回は既存 `ui_main::grid_reclick_open_tests::saved_group_modal_blocks_real_image_cell_loading_and_ready_until_exact_cancel` のready確認assertion（`src/ui_main.rs:27329`、100×10ms poll）1件でexit 101。当時のworker途中状態は出力されておらず、原因をtimeoutと断定しない。sourceを変更せず単独0.22秒で成功し、`RUST_TEST_THREADS=8`での再実行はworkspace・vendorを含めexit 0／PASS（全lib同件数、ui_snapshot 146成功）。初回の失敗記録を消さず、製品側の待ちやテストの判定条件は変えていない。AV再発なし。
- dev build完了（2026-10-10）: `scripts/build-dev.ps1 -PreserveRuntime` exit 0、通常機能セットの `target/dev-runtime/mimageviewer-core.exe` を更新。製品起動・commitなし。実機再確認は同じ7-plugin構成で `--perf-log` 起動、1分ほど通常操作、診断ZIP保存と正常終了を行い、保存済みcheckpointの通常／perf累計欠落0とUI応答を確認する。
