# §1.356 通常ログ・perfログの非同期writer 設計案

2026-10-09、ライン C。**設計のみ。製品コード未変更、実装前の独立レビューと下記の利用者回答待ち。**
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
| perf | `src/perf.rs:39/:58` のFILEとinit_with_path。mkdir／rotation／open、`:262/:270` のmutex＋write、`:287/:301` の明示flush | 同じwriterが別sinkとして所有。perf無効時は従来どおり即return |
| UIの通常記録 | `src/app.rs:87308` のheartbeat。perf有効時の `:87578/:87619/:87637`、通常描画末尾 `:89789` → `:8728` | 発生時刻・tidを採取して非blocking受付だけ |
| startup VST | `src/app.rs:27637` のload worker、`:27761` のUI try_recv、`:27773` の200ms repaint | load／poll設計は維持。ログのための待ちだけ除去 |
| VST／EffeTune reader | `src/video/dsp/bridge.rs:618–625` のstderr pump → `src/video/dsp/mod.rs:1146` の通常logger callback | callbackがwriter I/Oを待たずstderrを読み続ける |
| 現行slow_io診断 | `src/perf/stall.rs:158–166/:238–263` は独立有界メモリqueueへ測定結果を入れ、perfファイルguard下で排出 | writer内のwrite／flush／rotation測定へ移す。producerが旧FILE mutexを待つ構造は撤去 |
| 通常終了 | `src/lib.rs:1854–1873` がRemote／collectionを終了させた後perf::flush。`:1883`／`src/main.rs:5` が起動診断terminal | runtime終了後に一度だけlogging shutdownを完了させる |
| tray | `src/tray_integration.rs:118–123` のQuitは通常close／on_exit経路。hideとquitは別 | hideはwriterを閉じない。Quitだけ通常終了境界へ |
| 早期exit | `src/lib.rs:1002–1008` のexit_startup、`:1904/:1918/:1938/:1948/:1955` のTRT smoke exit、`src/ai/trt_worker_runtime.rs:63/:78/:162` のexit | 初期化済みloggerのbounded shutdownをexit直前へ。未初期化はno-op |

初期化は通常 `src/lib.rs:1228` → `:1256` の順だが、facadeの単独使用／遅いperf有効化（`:1399–1404`）もある。どちらから開始してもprocess内にwriterは一つだけ。`logger::init_for_worker` の別ファイル／append契約（`src/logger.rs:111`）を保持する。

### 2.1 最初の「起動中…」とStore関連

`src/ui_startup.rs:5–22` のrenderer自体はmemory-only。`src/app.rs:87681` がoverlayを描く**前**に、通常heartbeat（`:87308`）、perf有効時のframe.begin（`:87578`）、最初のupdateのstartup.first_frame（`:87619`）、初回から実行するperf::flush（`:87632–87637`）に到達する。creatorの `src/lib.rs:1681/:1704/:1714/:1759/:1845` → `:429` もperf有効時は同期writeである。通常loggerは常時ON（`:1222–1228`）、perfはCLIまたは設定で有効になる。
従って、現行コードには初回overlayの描画前に同期ログI/Oで止まる経路がある。本変更はその経路のファイルI/Oを外す。旧perfのfirst_frameはupdate入口の印であり、実PRESENTを意味しない。常時起動診断のGPU／Present観測（`src/lib.rs:1482–1509`）は別経路として維持する。Storeの観測ログがないため、今回の16秒停止とStoreの報告を同一原因と扱わず、審査問題の修正完了とも書かない。

## 3. 単一writerと非blocking受付（利用者回答待ちの推奨案）

### 3.1 所有と初期化

process内に一つの `LogWriter` を置く。通常sinkと任意のperf sinkの **mkdir、open、write、flush、close、copy、rotation、retention** はこのthreadだけが行う。facadeは時刻基準・出力先などのmemory設定を渡すだけで、UI上の初期化でもファイル存在確認・open・writer ready待ちはしない。
writer開始前／sink準備中にも有界queueへ受付可能とする。状態は単一の `NotStarted / Running / Closing / Stopped / Failed` 所有にまとめ、facade別のready／pending／stopped boolを増やさない。Runningは受付可能を意味し、ファイルopen完了を意味しない。通常／perfのsink状態はwriter内だけに保持する。perf後発有効化・override pathも一度だけ同じownerへ設定し、複数writer／重複truncateを許さない。
perfのsession.startは、producerがpid／process_start_unix_ms／wall clock anchor／t／tidを採取して初期化設定に保持する。通常queueの満杯で捨てず、writerはperf sink準備後の**最初の行**として書く。その後に当該sessionのperf recordsを出す。idle-healthの同session証明を遅い書込み時刻で作り直さない。

### 3.2 queue、順序、欠落

推奨容量は通常＋perf合計 **8,192件、payload合計8MiB、1レコード256KiBまで**。制限はすべて同時に適用。制限値は内部定数とし、設定画面／DBキーを増やさない。
producerは現在のメッセージ／JSON構築を維持し、enqueue前に自身のelapsed timestamp、OS thread ID、call siteを採取する。writer時刻／writer tidへ置き換えない。queueは先に確保した有界journalとし、memory-onlyの短いcritical sectionへ**一度だけtry_lock**して受付する。writerも固定件数のbatchを取り出し、guardを解放してからserialize／I/Oする。try_lock失敗時のsleep／再試行／通常loggerへの同期fallbackは禁止。
8MiBのpayload会計はqueueから取り出した未処理batchも含み、writeまたは破棄が終わるまで解放しない。固定journalの管理領域、各sinkの固定buffer、session初期化設定は別の固定上限を持つ。sink I/O中はmemory journalもbyte会計のguardも保持しない。
満杯、byte上限、oversize、memory競合時は**新しいレコードを捨てる**。既存queueの古いレコードを追い出さず、採用済みレコードは受付FIFO、同一threadの順序を維持する。異なるthreadの時刻順は保証しない（既存mutexでも時刻採取と取得順は一致しない）。oversizeはメッセージを途中で切ってJSON／既存書式を壊さず、件数に含める。producerの一時的なformat領域はqueue容量とは別であり、無制限入力の新しい収集を足さない。
sink別・理由別のAtomic dropped counterを持ち、writerが復帰したとき／flush／shutdown時に差分と累計を**queueを経由せず**直接記録する。通常は既存prefix付き `[async-log] dropped ...`、perf有効時は `cat=log, kind=records_dropped`（count／total／sink／reason）を追加。既存 `diagnostic_dropped` と別kindにして意味を変更しない。writer内部の自己ログはpublic facadeを呼ばず、再帰しない。ディスクが永久停止／processが強制終了した場合、そのcounterまでディスクへ残せるとは保証しない。

### 3.3 flushとslow_io

`perf::flush()` は **非blockingのflush要求**へ変更する。UIの1秒周期、手動診断のflush、初期化直後のflushを含め、呼出しが戻ってもwrite完了を意味しない。要求はqueue満杯で失われないcoalesced controlとして記録し、受付済みレコードのwatermarkまで排出後にwriterがflushする。後続の記録が絶えなくてもwatermarkでflushできるよう、無限のqueue空待ちにしない。flush完了待ちが必要なtest／終了処理は別のbarrier APIを使い、描画経路から呼ばない。
writerが採用したwatermarkはそのflushが終わるまで固定し、追加要求は次のflushにまとめる。初期化のLogger／perf stageは受付とwriter開始要求までの時間になり、open完了と誤表示しない。実際のsink open開始／完了はwriter側診断として別に記録する。
通常sinkもbuffer化し、推奨 **250ms周期**（記録があるときだけ）と明示flush／shutdownでflushする。アイドル時の新しいUI repaintやログ記録を作らない。周期待ちはwriter側の通知付き待機であり、producer待機ではない。flushはOSへのwriteであってsync_allによる電源断耐性の保証ではない。
既存slow_ioのkind／既存fieldは保持するが、今後はwriter上のI/Oとしてtid／holderを記録する。producerのcaller情報は追加fieldで伝え、UIが16秒待ったかのような旧caller tidの偽装はしない。queue residenceは別kind `queue_delay` とし、既存write_msへ足さない。診断はslow-onlyかつ有界、1レコードごとの自己診断を増やさない。既存解析の区間推定がwriter I/OとUI停止を混同しないよう `scripts/analyze_perf.py` のhitchesも同時確認する。

## 4. 独立して残す診断と子プロセス

「失ってはいけない」は通常queueのdrop対象へ移さないという契約。ディスク故障、強制kill、native crashでも必ず保存できるという保証ではない。

| 診断 | 現行根拠 | 決定案と理由 |
| --- | --- | --- |
| Rust panic／panic.log | `src/lib.rs:504–524` は通常logger→panic.logの順。`:532–551` は独立open／append、4MiB＋.bak | **panic.logへ先に直接同期記録**し、その後通常ログへbest-effort enqueue。一般queueをdrain／joinせず、writer mutexに触れない。unwind中やqueue満杯でも主証跡を通常loggerへ依存させない。panic hookからshutdownを実行しない（worker panicでprocessが続く場合もある） |
| native exception | `src/lib.rs:865–886` は既存例外情報をpanic.logへ記録 | 独立経路を維持。新しいalloc／lock／writer待ちを足さない。既存native handlerの限界は本件で拡張しない |
| UI hang watchdog | `src/lib.rs:571–578/:634–638` は専用threadからpanic.logへ直接記録 | panic.log直接同期を維持。通常ログ複写はenqueueだけ。native-healthもpanic.logを先に書く。UIや一般writer停止中でも別の証跡を試みられる |
| 常時startup JSONL | `crates/startup-diagnostics/src/lib.rs:212–231` の非blocking journal、`output.rs:32–40/:105–140` の独立writer、`lib.rs:654–697` のterminal | **別owner／writerのまま**。一般writerへ統合せず、通常／perf queueが停止・満杯でもstage／watchdog／terminalを公開できる。既存容量・drop／retention／独立watchdog契約を保持。終了時のterminal排出ACKは§5の共有境界で補完する |
| Remote service log | `src/remote_ipc/service.rs:440–448` はRemoteへ--logを渡し、別processが `crates/remote-web/src/diagnostics.rs:16/:150–161` でrequest JSONLを同期保存 | **Remote側の専用ログは現状の同期のまま**。本体UIのwriterとは別process／別ファイルで、秘匿値除去・request schema・retentionを維持。Remote自身のI/O応答性改善は本件に混ぜない。core側stderr monitor（`service.rs:496–503`）の通常logは今回の非blocking facadeで改善 |
| PDF child | `src/lib.rs:1106–1114` はlogger初期化前にworker分岐。`src/pdf_loader.rs:2949/:2913–2929` はstderr pipeとreader→通常logger | childの同期stderr出力／IPC framingを維持し、parent readerだけログI/Oを待たなくする。childがmain logを初期化／truncateしない契約を保持 |
| Susie child | `src/susie_loader.rs:1072` のstderr=null、`crates/susie-worker/src/main.rs:34–38` のstdin／stdout | 新しいstderr収集／logファイルを加えない。parentの通常診断のみ非同期化。stdoutはbinary IPCで、log queueへ流さない |
| VST3／EffeTune host | `bridge.rs:618–625` のreader。`crates/vst3-host/src/main.cpp:1301–1302/:1323–1327` のstderr printf／fflush | C++ stderr／protocolは維持。parent排出を非blockingにする。child stdoutの制御応答をログdropで失わせない |
| TRTなどlogger使用child | `src/ai/trt_worker_runtime.rs:31` はinit_for_worker、`:63/:78/:162` は直接exit | facade共有なのでprocessごとのwriterを使用し、専用append先を維持。直接exit前にbounded shutdown。pdf／Susieと同じ初期化と決めつけない |
| EPUB／その他reader | `src/epub_convert.rs:569–572` のstderr→logger | 同じfacadeを使うreader／workerを横断監査。IPC結果／進捗のchannelは変更せず、stderrのdiagnostic複写のみ対象 |

### 4.1 allocラインの .lock sidecar と取り込み境界

利用者指定のstartup JSONL **`.lock` sidecar修正を維持する**。ただし基準HEADでは `crates/startup-diagnostics/src/output.rs:61` がJSONL本体をshared lockし、`:228` のretentionも本体をexclusive lockするコードであり、sidecar修正はまだこのHEADに無い。これは取り込み順の依存である。allocラインのcommit／契約をcoordinatorから取り込み、同じsidecarをwriterの生存証明／retention判定に使うことを確認してから実装・検証を完了する。他worktreeを編集しない。本件でJSONL本体lockへ戻す変更や独自の二つ目のlock方式を作らない。sidecarの既存schema／寿命／cleanupはalloc側仕様を正本とする。

## 5. 終了、crash、まれなI/O失敗

1. 通常close／tray Quitは既存のsettings・sidecar保存、App／Remote／collection／child終了を先に行う。App::on_exit内で一般writerを止めず、終了workerの最後のログを受付できるままにする。tray hideでは何もしない。
2. run_nativeとruntime cleanupの後、起動診断へ実際のterminal outcomeを公開する。通常logのshutdown開始印を受付後、一般ownerをClosingへ一度だけ遷移。producerはjournal取得後にもClosingを検査し、writerが取り出し中のbatchとqueueを含め、Closing前の採用分を排出する。遷移時にcritical section内だったproducerとdrainの順序をmemory境界で確定する。Closing後の通常レコードは採用しない。panic.log／startup terminalはこの受付制限の外側。
3. workerはdrop集計→両sink flush→close→完了通知。shutdown呼出元は**最大2秒、一般writerとstartup writerを合わせて一つのdeadline**で完了を待つ。待機をwriter数だけ積み重ねない。成功なら完了済みthreadをjoinできる。timeout／writer failureならjoinやFile／BufWriterのdropを呼出元で行わず、threadとI/O所有を残してprocess終了へ進む。再flush／同期fallback／retryはしない。これは終了時間の上限であり、UI停止をtimeoutでごまかす修正ではない。
4. `src/main.rs` の通常return、`exit_startup`、scripted exit、TRT worker／smokeの直接exitを同じidempotent終了helperへ接続。writer未起動は即return。終了後も生きるwatchdog等は一般writerを再作成しない。loggerを使う開発binのreturn／exitも棚卸しし、独立binaryのログtailを失わせない。
5. panicでは先に直接panic.logを残す。unwind後に通常returnへ到達する場合だけ通常終了helperを通す。abort／native fault／TerminateProcess／電源断はdrainを保証しない。panic hookで全queueを直接書く方式は、停止writerと同じdiskを待ち、二重所有／再入deadlockを増やすので採らない。

startup writerのterminalは現行 `lib.rs:654–697` がpublish＋wakeだけで、flush完了ACKはない。また `output.rs:105–145` はbatch処理後にStoppedを見て戻るため、64件を超えるpending全体の排出保証でもない。**必要な最小共有変更**として、独立ownerが採用済みterminalまでのpendingを排出し、最後のflush／close後にACKする終了契約をstartup／alloc担当と合意する。容量上限到達／sink failureは成功ACKと区別する完了結果だけを返し、再試行や新しいqueueは足さない。通常publish／watchdogはACKを待たず、core／launcherの最終終了境界だけが同じbounded deadlineで待つ。一般writerがstartupファイルをopen／lock／flushする変更はしない。取り込み時にterminal順序・.lock sidecar寿命・retentionの担当をcoordinatorが確定し、既存terminal testに排出完了testを追加する。先行変更だけでACKがあるとは扱わない。
terminalの最終1件は独立ownerの終了metadataとして保持し、通常journalの満杯／try_lock競合で失わない。writerが既存schemaのterminal行を末尾へ一度だけ出す（従来queue内との二重出力は禁止）。ファイル上限にもその一行分を予約する。途中stage記録の既存drop上限は維持し、通常queue全体を無損失化する仕組みは追加しない。この小さい保護とACKをstartup側の同じ終了ownerに置き、一般loggerに複製しない。
open／write／rotation失敗はaffected sinkをそのprocess中Unavailableにし、自動再open／retry／回復queueを足さない。他sinkとUIを続け、memory statusへ一度だけ失敗を公開し、次の通常UIで「診断ログを保存できません。次回起動時に再試行します」を一度通知する案。次の起動で通常初期化を試す。失敗通知を失敗sinkへ書く再帰を作らない。shutdown timeout時は新しい同期panic.log書込みも試さない（上限を破るため）。未flush tailの欠落は利用者質問C356-3／4／5で明示承認を得る。

## 6. 状態の簡素化と互換性

- ログ記録中をモーダルにする案は不採用。観測処理が閲覧／IPCを禁止する機能変更になり、disk stallをUIへ戻す。一つの一般writer、固定上限、drop-new、終了時の一回だけのsealにより組合せを減らす。
- per-thread writer、sinkごとの一般writer、無制限queue、優先度別queue、古い記録の退避、再試行／disk spool、panicから一般queueをdrainする案は不採用。通常／perfの片方が止まると両方の保存は遅れるが、producerが止まらず、重要診断は独立経路なので、一つのownerで足りる。
- released-data: 通常／perfログはv4.4.0以前から存在する**リリース済み診断形式**。設定・タグ・collection等の利用者データではないが、解析toolの入力なので既存書式を維持する。DB schema／設定migration／既存データ削除は無い。
- 通常prefix `[elapsed s][tID]`、perfのt／tid／cat／kind／key／seq／extras、session.startのpid／wall anchor、時刻基準、UTF-8、改行、pathを維持。writer受信時刻でstampしない。新kindは追加だけ。`scripts/analyze_perf.py:62–80` のJSONL readerと、`scripts/check-idle-health.ps1:228–250/:299–319` のpath／session証明にfixtureで照合する。
- 通常は現行16MiB・1 rotated generation、debug retentionは256MiB・4 generation、起動時.prev保存（`src/logger.rs:13–22/:120–134`）を維持。perfはcurrent＋過去4世代、override pathはrotation無し（`src/perf.rs:44/:56–93`）を維持。新workerが既存ログ以外をretention対象にしない。panicは4MiB＋.bak、startup／Remoteの独立retentionも維持。
- 実装時のdoc co-update: `async-architecture.md` §worker一覧／§7、`ui-responsiveness.md` のログ診断境界、必要なら `idle-health-check.md` とmanualの診断ログ説明。保存先／通信先は変えず、製品ページ・privacyの保存データ説明も整合を確認する。文言・glyph／UI表示が変わる場合のみsnapshotも追加する。

## 7. 検証計画（実装時、実アプリ起動無し）

| 検証 | 採用基準 |
| --- | --- |
| 有効なred／実経路のstall | fake sinkのwrite、flush、open、rotationをそれぞれlatchで止める。logger／perf facadeとfake stderr pumpを通し、sink未解除のままUI役／reader役producerが全件処理を終えることをchannelで検査。現行同期版は解除前に完了できないことをredとして記録。時間sleepでlockの状態だけを見るtestにはしない |
| bound／drop | 件数／bytes／oversize／journal競合で非blocking drop。上限不変、採用済みFIFO、回復後のdrop差分／累計、perf OFF時も通常summary、summaryが満杯queueに再投入されないことを固定 |
| timestamp／ordering | 複数threadから既知stamp／tid／連番を投入。各threadの採用済み順序を維持、writer tidで置換無し、同threadの正常系を無欠落で検証。別threadのwall-time sortは要求しない |
| init／session／flush | 通常のみ／perfのみ／後発perf／override／重複initでwriter一つ。open中にも受付可能。session.startが先頭かつ満杯で消えない。UI flushがfull queueでも戻り、watermark前をflushして後続無限流入で飢餓にならない |
| rotation／retention | disposable dirと小さいtest上限で.prev／.bakN／perf世代／overrideを実ファイルで検査。unrelated files・設定等のhash不変、起動中read共有、rotation後も順序／完整JSON行を検査 |
| shutdown | 採用tail／取り出し済みbatch／drop summaryまでflush後ACK。Close／tray Quit／早期exit／scripted／worker exitが同じhelper、hideは停止無し。fake clock／completion channelで最大2秒を検証し、未完了threadにjoinせず、timeout後producerがI/Oへfallbackしない |
| 重要診断 | writer永久stall＋queue fullでpanic／watchdogのfake direct sinkへ先に記録できる。catch_unwind後にprocess状態を壊さず、native経路へ新待ち無し。startup publisher／watchdogは一般writerから独立、terminalはfull／競合／pending>64件／ファイル上限でも予約した最終行を一度だけ排出してACK。startup sink自身のstall時は合計deadlineを超えず、allocの.lock retention testは維持。Remote secret-redaction／request logは維持 |
| 失敗・解析 | sink failure時は一度通知、他sink継続、retry／再帰無し。既存v4.4.0 fixtureと新writer fixtureをanalyze_perf（startup／hitches／pre-grid等）へ通す。idle-health parserのsession／既存分類をheadlessで検査（scriptのapp起動経路は使わない） |

focusedで有効なred→greenの後、`cargo fmt`／`cargo fmt --check`、`cargo test -p mimageviewer --lib`（pipe無し・実exit）、normal／portable `cargo check -p mimageviewer --bin mimageviewer-core`（portableは `--features portable`）、`python scripts/check_ui_glyphs.py`。startup crateも単体testし、UI失敗通知を変更するならui_snapshot。共用loggerの横断変更なので最終 `scripts/test-full.ps1` はcoordinatorと担当を決め実施し、valid evidenceを共有する。依頼された実装時にはbuild-devを作るが起動しない。現在のroundは文書のみなのでbuild／test／実機検証は行わない。
利用者の実機確認は、coordinatorがbuild後に手順を渡す。起動・VST復元中の操作、perf ON/OFF、tray Quit、診断の保存と欠落summaryを確認する。重い並行build/testは観測条件だが、負荷を掛けた実機確認は別途利用者と計画し、こちらでは実行しない。

## 8. 利用者への質問（すべて新規、未回答）

1. **C356-1 容量**: 通常ログとperfを合わせ、最大8,192件・本文合計8MiB・1件256KiBでよいですか。**推奨: はい。** メモリが増え続けない上限です。256KiBを超える通常記録は捨て、panic全文は独立panic.logへ残します。
2. **C356-2 捨て方**: 満杯やqueue競合時は新しい通常／perf記録を捨て、保存が再開したときに欠落件数を記録してよいですか。**推奨: はい。** 起動直後の採用済み記録を残し、画面やstderr読取りを待たせません。panic／watchdog／常時起動診断はこのqueueへ移しません。
3. **C356-3 保存間隔と終了**: 通常は250msごとにflushし、実際の終了時だけ通常／perfと起動診断を合わせて最大2秒待ち、それでも書けなければ残りを諦めて終了してよいですか。**推奨: はい。** 強制終了では直近の未保存ログが失われ得ます。画面上のperf::flushは要求だけになり、完了を待ちません。
4. **C356-4 crash時**: panic／UI hangは独立panic.logへ直接書く一方、crash時に通常／perfの未保存queue全体を書き切る保証は設けない方針でよいですか。**推奨: はい。** 止まったwriterをpanic処理から待つ危険を避けます。panic.log自体もディスク故障や強制killでは保存を保証できません。
5. **C356-5 まれな保存失敗**: ログ保存に失敗したら一度通知し、その保存先は次回起動まで使わず、再試行を増やさない扱いでよいですか。**推奨: はい。** 閲覧と他の診断先は継続し、利用者が作った設定やタグには触れません。

決定記録: **2026-10-09 利用者: 本件を次の版・ラインCへ入れる。今回は設計のみ。** 上記の数値・欠落・終了方針は、この決定で回答済みとは扱わない。
