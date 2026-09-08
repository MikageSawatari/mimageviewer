# 本照会 R1 / R5 / R6 の実装引き継ぎ

2026-09-08。親 Astra / high が設計・進行、Sol / xhigh が実装・テスト、独立 Astra / high がレビューする。
本書は [レビュー修正記録](duplicate-detection-review-fixes-20260907.md) に蓄積した現合意を整理したもの。
撤回した案を再採用せず、矛盾は実装前に根拠とともに親へ戻す。**製品実装と採用判定は未完了**。
R4 の全体テスト・portable 作成と更新照合が完了し、独立owner/executorの第1区切りは3109b60e6で保存済み。
需要状態と完了通知もbe0075dc1で保存済み。22件成功後、通知fixtureだけ同期を補強し対象1件が成功した。独立coreレビュー通過。
global dispatch gateはad2130581、専用readonly reader第1区切りは5e0d2bd1bで保存済み。各狭域回帰・製品check・独立レビュー成功。既存query callerと本検索計算はまだ切り替えていない。

## 修正対象と維持する性質

- R1: PDQ 半径32の一致は推移的でない。候補側の common 調査を省くと、判定不能な本を「同じ」と過大評価する。
- R5: 256ページの hit 上限は、8冊を超える common の証拠にならない。
- R6: 配列・SQLite・ページ順を異なる時点から組み合わせない。
- 単体画像の検索と cache は維持し、本照会だけを変更する。表示機能の削除、ページ間引き、hit 打切りは行わない。
- 利用者は音声途切れの解消を確認済み。高負荷の全走査を単純に復活させない。既存の低優先度実行を維持する。
- 通常設定と既存 portable の DB・配列を再生成・初期化しない。実データ計測は read-only または一貫した検証用 backup を使う。
- §9.2 の横断一覧、master への逆統合、push は今回の範囲外。

## 実装の区切りと責任

1. viewer client と公平な実行所有: `SimilarPanelState` → `App::query_similar_book` → manager / worker。
2. 一つの read transaction で完結する DB reader と配列追随。
3. worker が所有する厳密半径検索 MIH。
4. common / 候補発見 / 分類と alignment / ページ帯、および oracle・負荷検証。

各区切りで主要前提を既存コードと照合し、独立レビューと狭域回帰を入れる。
新 worker が取消・終了まで接続される前に製品の呼出先を切り替えない。
DB は commit 結果を値で返し、scheduler が通知する。DB から query owner へ依存させない。
worker から manager への強参照循環や、Connection と Transaction の自己参照構造を作らない。
同じファイルは Sol だけが編集し、親は設計文書を担当する。

## 公平な要求所有

- global の実行 worker は1件。既存の viewer bundle 内 `SimilarPanelState` が `BookQueryClient` を所有する。
- client ごとに要求ID、desired / requested / completed、取消、完成結果、最新待機1件を所有する。
  完成済みの同一要求を毎 frame 再投入しない。同一本を要求する2窓も別 client。
- 待機は FIFO / round-robin。同一要求の poll は順番を変えず、soft refresh の最初の投入は列末尾。
  既存の待機 refresh があれば最新1件へその場で集約し、連続更新で順番を毎回移動させない。
- client の origin 変更・撤回・drop はその client だけを取消す。park / raw swap は client を失効させない。
- 完了・失敗・取消の終了回収後は、UI polling を待たず次 client を実行する。
- `query_book(client, container_key)` 相当へ接続する。公開 API の引数になる client は opaque な公開型にできることを確認する。
- `current_item == None` / NotBook の早期 return でも当該 client の要求を撤回する。
- 起点は本の key。Item 用 `begin_origin` のページ key 変更を、そのまま本照会の取消キーにしない。
- 既存 `last_ready` は ItemQuery 用。本の直前結果が既に保持されているという前提を置かない。

scope / store / shutdown と要求 origin の変更は hard invalidation。
同一 store の配列更新・compaction は soft stale とし、整合した実行中 TX の結果を一度公開し、refresh を集約する。
同じ hard identity の旧 Ready は refresh の待機・実行中にも取得できるよう保持する。
`requested` は実行中 ID を維持し、soft 更新は wanted と待機だけを更新する。完了採用は実行 ID と
hard identity の一致で判断し、wanted ID 一致を要求して実行結果を捨てない。
A実行＋B待機中の連続soft更新では、Aの結果を採用した後、UIpollを待たずB→A refreshへ進む。
hard origin/scope/store更新はReadyを即退役し、A→B→Aでも旧完了が復活しない要求世代を使う。
現 `BookQueryState::Idle` 代入を一律置換せず、`configure`、scope、run 終端、配列公開の意味で分類する。
第1区切りの接続では現 `Arc<Mutex<BookQueryState>>` を置き換える。次を明示してから切り替える。

- 現 scheduler shutdown は index cancel だけなので、manager Drop から新 book owner の停止・wake・終了回収を接続する。
  Drop は signal/cancel/wake までとし、UIで join・DB待ちをしない。worker側の停止と資源解放を回帰で観測する。
- `retain_loaded_snapshot_during_run` は roots configure による hard、`publish_snapshot_if_current` と
  run の正常終端は soft。`start_memory_load` の Ready / Missing / Failed 公開も待機 worker を起こす。
- `SimilarPanelState` は true close でも drop されない。実 close で当該 client を明示撤回し、
  bundle park / raw swap では保持する。client Drop だけを close の取消条件にしない。
- park でも使う `invalidate_similar_preview` へ本照会の取消を混ぜない。
- scheduler は現 state lock 中に旧 owner へ到達する。新 owner の lock から scheduler / memory / DB を
  取りに戻らず、通知・dispatch は guard 解放後に行う。

先に opaque client・公平 owner・executor の終了/取消を回帰で確認し、次に manager / scheduler 通知と
viewer 撤回を一緒に接続する。旧分類器を一時的に executor へ移す段階は R1/R5/R6 の判定修正完了とは扱わない。
先行 module は `src/similar_book_query.rs` を想定し、独立レビューで次の境界を合意した。

- client は移譲可能・非Clone。初回登録のWeakとexecutor identityを照合し、別managerへの投入で旧登録を残さない。
- 実行権は global `Running(client_id, request_id, cancel)` だけを正本とし、client側はそのidentityを参照する。
- Condvarはlock下で状態述語を再確認するloop。未起動・常駐idle・job実行を区別し、lazy spawn失敗もterminal化する。
- 受付/停止はworker lifecycle enumから導き、shutdown boolを重ねない。clientの未投入/待機/実行ID参照も
  単一phaseへまとめ、未bind/登録済みは一つのbinding enumとする。completedはrefreshと共存する独立結果cache。
- refresh世代はownerが通知受付時に単調発行する。外部DB/配列連番をIDとして渡さず、遅延通知で
  global世代や新規clientのdesiredが後退しない。DB/TX/配列の世代選択はruntimeの別契約とする。
- shutdownは受付拒否・待機退役・実行取消・wakeを行い、job終了後にworker-local資源をdropする。
- reader/MIHを将来のworker-local runtimeへ置けるAPIとし、client/待機jobに巨大snapshotやmanager強参照を捕捉しない。

純mock段階で公平性・supersede・withdraw/drop・失敗後dispatch・shutdown回収を検証する。
旧query計算にはまだ取消点がないため、通知・viewer lifecycle・取消可能な実計算を揃えた後を製品caller切替の完成境界とする。

### caller接続の追加監査と要求範囲の再評価

独立UI監査で、current_item=Noneはmetadata panelのOption::mapでhelper自体を呼ばず、非対象itemは
App helperでNotBookへ早期returnすることを確認した。可視Similarでこれらを受理する際は明示withdrawする。
DB計算の完成結果NotBookは別で、同一起点の完成結果として保持する。
通常true closeとfinish_fs_navigation_sequenceのViewerExitedは明示withdrawし、had_navigation_ownerの有無へ依存させない。
旧ページを維持するscan失敗/Supersededは保持する。retire/build abortのpayload Dropは当該clientだけを退役する最後の保証。
manager早期return前にbinding/desiredを更新する。

hidden/Info中の物理viewer起点を常時追従する案は、元R1/R5/R6への追加要求だったため、親と独立coreが範囲を再評価した。
要求originは**そのclientが最後に実際に受理したqueryの本**とする。非表示中の旧本完走は同じ論理要求の完了である。
再表示の実queryでは既存shown_items.first()による最新keyを先に受理し、hard照合した戻り値だけを描く。
これで別本のReadyを公開せず、非表示の全contextへshown-origin監視を追加する必要はない。

現在のresolve_spread_pairはcache miss/recheckでrotation_db.get_manyへ到達し、全AtRest巡回へ広げると同期IOを増やす。
旧layoutやSingleへのfallbackは起点の意味を変える。これらを新しい監視用に拡張・代用する案は採らない。
物理item代入の瞬間からworker内部cacheの旧完了writeを禁止することも、最後に受理した要求を所有する契約には不要。

ただし現executorの全client soft再投入をそのまま残すと、非表示中も索引更新のたびに仕事を作り、元挙動よりCPUを増やす。
独立coreと次の需要型を合意した（第1sliceへの追加実装・UI需要境界の監査は後続）。

- container_key OptionをDemand::{Withdrawn, Active{key}, Retained{key}}へ置換し、hidden boolを重ねない。
- Retainedは既実行/既待機1件と完成cacheを保持する。softはdesired世代だけ更新し、新規enqueueしない。
  Retainedのfinishもrefreshをenqueueしない。既待機のFIFO位置は変えない。
- Active復帰は実queryで最新keyを受理してから行う。visible通知だけで古いkeyを再投入しない。
  同keyなら旧Readyを取得しながら最新refresh1件を要求、新keyならhard切替する。
- hard scope/storeはRetainedを含め既仕事取消・cache退役。置換jobはActiveだけをenqueueする。
- trueclose/dropは需要に関係なく即withdraw。park/raw swap/AtRestへの格納は実UI非表示と同義ではないため、
  それだけで需要状態を変更しない。client identity/cache/既受付仕事はbundleとともに保持する。
  一方、実際にfrozen表示へ移る意味的passive化では旧panel判定が再実行されないためRetainedが必要。
  既存transition_detached_window_stateでto=Parked/ParkedLive確定後、window_idの既存bindingから
  当該clientへ到達してretainする。Closing経由もあるためfrom=Active限定にはしない。binding無しはno-op、
  mounted clientへのfallbackはしない。Active/Resuming遷移だけでは需要を復帰させない。
  snapshot失敗前にも走るreset_detached_pause_foreground_modesへ便乗しない。predicate/viewport/placementは
  変更せず、接続時にdetached plan§11へ意味的遷移と所有境界を記録する。
- 実panel/tab/windowの需要変更はIO不要のretain操作へつなぐ。描画されないframeを即非表示と誤認しない。
  独立UI監査で、draw_metadata_panel_innerの実visibility false・tabs後Info、fullscreenの実panel選択blockが
  metadataを抑止した場合、通常panel blockを通らないnative presenter委譲分岐をRetained入口として確認した。
  音声/編集/分析/panoramaも実panel選択判断に合流する。native overlayへclient/commandを持ち込まない。
  未描画frame/park/raw swap/一時borderless移行/startupの一時returnは無変更とする。
- closeボタン後も同passのqueryまで流れる現経路に注意。tabs処理後・shown-origin解決前に現在の
  fs_info_panelと既存visibility条件を再確認し、閉じた後の新query/Active復帰を防ぐ。
  非需要時の同pass描画を維持する場合、同identityの既completed参照だけを許し、受付を起こさない。

旧Ready保持中は現UIのPreparing限定request_repaint_after(200ms)が走らない。
soft完了やworker致命失敗をlock解放後にUIへ通知する境界（依存注入notifier等）、またはrefresh状態の返却を
adapterで接続し、操作待ちにならず新しい結果が表示されることを確認する。独立UIのsource監査では、
ROOTへの一度のrequest_repaint_of(ROOT)がactive viewer更新→所有bundle mount→show_viewport_immediateの
同期callbackを通り、可視detached本パネルにも届く。passive detachedはfrozen snapshotのみで別通知は不要。
generic executorへFnを注入し、App/manager層でctx cloneを捕捉する。完成/致命失敗の公開とlock解放後に呼び、
古いviewport IDや追加timer/loopを持たない。この経路はsource監査済みで、実行検証は後続。
第1sliceの純executorへegui型を持ち込む必要はない。
### 未ロード時の受付と実storeの識別

草稿でOriginへraw store_idを必須にすると、現Unloaded/LoadingでPreparingを返す受付と衝突する。
親・独立coreはclient要求を本key、requestのhard identityをowner内部のopaque環境世代へ分ける方針を承認した。
第1区切りへraw DB型を入れず、初期ロード中もdesired/FIFOを保持する。memory Ready/Missing/Failedの通知で
再開または終端化するadapterは第2区切りで接続する。UIでDBを開いたり、ゼロstore IDを未確定sentinelにしない。

実store IDはruntimeが同TXで観測し、未確定/確定は型で表す。
配列のstore不一致はsnapshot候補の棄却と同TX memory-only fallbackであり、既知の実DB store変更と区別する。
shared base X・DB Yの反例では、Yを実storeとして確定後に旧base Xを見るたびhard世代を進めてはならない。
それを行うとYの要求を永久に取消す。実DB storeの変更時だけhard失効し、配列選択はruntimeの整合条件で処理する。
### 初期ロード待ちとglobal dispatch gate（第2区切り前提）

独立core監査により、Preparingをcompleted terminalとして保存する案は採らない。
初期A/BのFIFOを消費してから単にwakeすると再dispatchされず、Deferredを末尾へ戻す案も順序変更とlost wakeを招く。
推奨境界はglobal gateとowner通知ticket。owner lock下でticket/FIFO/shutdownを読む→guard解放→readiness probe→
ownerを再lockしticket不変なら待機、変更済みなら再probe。待機中はFIFOを消費しない。
publisherは正本を更新し全guardを解放してからticket更新/wakeする。probe直後・wait直前のReady通知も取り逃さない。

Memory Unloaded/Loadingでもloaderの開始・進行義務がある間は待つ。表示用IndexProgress::RunningはDB open/repair後に
立つため、開始義務の判定にはscheduler worker_running等の実所有を照合する。probeからfallback loaderを始めない。
初期open中にBookQueryFallbackが先行してMissingを公開すると、Unloaded限定の後続loader開始を妨げる反例がある。

indexer終端後にもUnloadedでproducer不在なら、無条件の待機では進行しない。
この限定終端では共有snapshotなしで照会可能として、同TX read-only readerとmemory-only fallbackへ進む方針。
実DB無しはNotIndexed、読取失敗はFailedへ終端し、queryからmutable load_or_rebuild/pruneを始めない。
具体的なMemory/scheduler状態射影は実装担当が着手前に検証し、暫定案の全Unloaded待機を再採用しない。

通知はload Ready/Missing/Failed公開だけでなく、load spawn失敗、index worker正常/取消/失敗終端、
finish_workerのDB open失敗等、spawn_worker失敗、scope configureのretain/unloadにも必要である。
memory epoch更新とprogress/state確定を済ませ、scheduler/state→book ownerのnested通知を全guard解放後へ移す。
旧epochのload結果を捨てる場合、旧結果として通知せず、その原因のscope変更側が再評価を通知する。
FIFO保持、probe直後のReady通知、Unloadedでのindexer失敗終端、待機中hard cancel/dropを回帰で確認する。
実コードの追加照合により、Missing/Failedをglobal gateのCompleteへ直接写像する案は撤回した。
start_memory_loadはUnloaded以外で開始せず、array_update_loopもReady以外の配列を更新しない。
ItemQueryFallbackが初期DB openより先にMissingを公開すると、indexerがDBを作成しても共有memoryにMissingが残り得る。
Failedには派生baseの読込/保存失敗も含まれ、いずれも現在DBの不存在・読取失敗の証拠ではない。

独立coreと合意した製品射影は次のとおり。

| memory状態 | dispatch |
| --- | --- |
| Ready | Run。共有snapshotは候補としてTX整合を検証する |
| Loading | Wait。実loaderの完了・失敗・epoch変更通知で再評価する |
| Unloaded、実loader開始義務あり | Wait。表示用Runningではなくschedulerの実所有を読む |
| Unloaded、producer不在 | Run。共有snapshotなしで同TX内のmemory-only fallbackを使う |
| Missing / Failed | worker_runningにかかわらずRun。共有snapshotなしでDBを読み、実不存在はNotIndexed、実読取失敗はFailed |

表示互換はcompletedと返却値を分ける。実NotIndexedをownerに保持し、有効keyかつ実scheduler稼働中の返却時だけPreparingへ投影する。
Preparingをterminalとして保存したり、毎frame再投入したりしない。実Ready・実読取Failed・OFF対象のNotIndexedは補正しない。
これは独立coreが承認した後続adapter接続の契約であり、先行generic gateはまだ製品UIを変更しない。

共有loaderの再試行設計や単体画像検索は、この本照会の区切りで変更しない。
runの正常/取消/失敗終端、finish_worker、spawn失敗はsoft失効とgate ticketを直接通知する。
request_array_refreshだけではMissing/Failed時にpublishが起きず、private readerの完成結果も再計算されないため、代用しない。

### generic gate先行区切りの実装合意

BookQueryDispatchGate<T>はRun / Wait / Complete(BookQueryTerminal<T>)を返し、製品DB型を持たない。
Weak通知口からownerのdispatch ticketを更新してwakeする。FIFO先頭はprobe前に消費せず、owner lock外でprobeし、
再lock後にticket・head・lifecycleを再照合する。Waitは同じ述語でCondvar待機し、Completeはruntimeなしで先頭要求を終端する。
Runが確定するまではruntime factoryも実行しない。worker localは未初期化/利用可/再生成待ちを単一enumで表し、製品callerはまだ切り替えない。

独立coreの条件: head比較はclient IDだけでなくkey/hard generationも含めるか、hard/origin変更側でticketを進める。
同clientのA→B→Aやglobal hardを取り逃さず、同じ要求の毎framepollではticketを進めない。
signalだけではComplete済み結果のfreshnessは更新されない。MissingをComplete(NotIndexed)にした後、Ready公開がwakeだけだと旧結果が残る。
soft_refresh / hard_invalidateは失効とdispatch ticketの更新を同じowner lock下で行い、製品publisherもこの統合通知を使う。
soft更新とsignalを別呼び出しにして、その間の古いprobe Completeを新refresh世代として保存してはならない。
古いComplete probeを停止→soft更新→再probeでRunとなる回帰を含める。signal単独は待機解除専用である。
Runはsnapshot permitを保持しないため、runtimeの再captureでLoading/UnloadedになっていてもPreparingをterminalへ保存しない。
hard取消なら破棄し、まだ有効なRunなら同TX read-onlyとmemory-only fallbackで実terminalまで進める。

回帰対象は待機中A/BのFIFO保持、probe直後の通知、Complete後の次client、wait中hard/withdraw/drop/shutdown、
Run前factory/executeが0、lazy化後のruntime init失敗・job panicからの再構築である。
初回gate回帰は28成功。独立レビューでruntimeのOption＋restart boolを単一enumへ直し、probeを停止する3回帰の失敗時回収を補強した。最終tests3も28成功、check成功、独立再レビュー承認。
probe停止中のsignal/soft/ABA操作は別threadで行い、bounded返却結果を保存してから必ずprobeを解放し、回収後にassertする。
将来probeをowner lock中へ戻す誤修正があっても、テスト自体を停止させない。

## 同一 SQLite read transaction

専用の read-only 接続を worker local に持つ。schema 更新を行う `SimilarDb::open_at` は照会 reader に使わない。
DB不在と権限・破損・schema/read errorは区別し、is_file=falseやCannotOpenを一律NotIndexedへ畳まない。
既存 SQL は private な `&Connection` helper へ共有し、旧 public wrapper の挙動を維持する。
`BookReadSnapshot` は reader の transaction 期間だけを借用する。

捕捉した配列・scope・要求から read TX を開始し、同 TX で store_id と変更連番を取得する。
BEGIN DEFERREDだけでは読取snapshotは固定されず、最初のmetadata SELECTで固定される。
先行実装はsimilar_db内のSQL helper共有・専用reader・with_snapshot要求scopeまでとし、MIH/classifier/manager callerは接続しない。
編集範囲はsrc/similar_db.rsとCargo.tomlのhooks featureに限定した。similar_search_arrayの非永続fallbackとZIP comparator/effective orderは次区切りであり、このreader保存の完了へ含めない。
変更連番 `read_seq` は履歴 prune 後も残る `sqlite_sequence` を読み、履歴の `MAX(seq)` で代用しない。
配列を同 TX の連番まで delta 追随してから、起点・候補・common・ページ帯をすべて同じ時点で解決する。
対象の identity、revision、signature、quality、Complete 状態、hash version、scope、page-order version も混在させない。

- store 不一致、履歴欠落、配列が TX より先なら、同 TX 内でメモリ上だけの compact base を作る。
- 本照会から persistent `load_or_rebuild` / rebuild / prune を呼ばない。
- `ItemChangeBatch` 自体に store_id はないため、別途 store を照合する。
- ページ順修復は item_change を増やさない。`worker_loop` → `repair_page_order_if_stale` →
  `renumber_container_pages` の commit 成功を `NoChange / Committed` 相当で通知する。
- 修復対象 container が0でも order-version の commit があり得る。件数0を理由に通知を省略しない。
- 同じ変更連番でも修復後の要求IDを更新し、起点・候補・common のページ順を同じ version へ揃える。

### 専用SQL readerの取消境界

ローカルrusqlite 0.31とbundled SQLiteの独立調査に基づき、hooks featureのprogress_handlerを専用readerへ付ける方針。
Arc<AtomicBool>をAcquireで読み、追加の監視workerや共有writerへのhandler登録は行わない。
get_interrupt_handleだけではidle時のinterruptが次SQLへ持ち越されず、flag確認からSQL開始までの取消を取り逃す。

handler closureはConnectionが所有する。要求ごとのRAIIでerror/panicも含め解除し、旧cancel tokenを次要求へ残さない。
statement/Rowsの終了、hook解除、readTX rollbackのborrow/drop順を具体的な実装で確認する。
SQLITE_INTERRUPT/OperationInterruptedは当該cancel=trueなら取消、falseならFailedとして原因を保持する。
SQL外のRustループには別途取消点が必要である。

SQLiteのbusy待機はprogress handlerやinterrupt flagを見ない経路があるため、即取消の保証とはしない。
新readerは接続既定の5秒を明示する案とし、schema移行用writerの180秒は流用しない。最終待ち時間と実測はreader採用時に記録する。
busy handler用の別所有構造や監視workerは先行gate区切りには追加しない。
隔離DBで長時間SELECT中の取消、同Connection次要求成功、error/panic後の解除、別writer非干渉を検証する。

### ページ順修復が失敗・未完了の場合

実装前の再照合で、現repairは失敗をlogして続行するため、旧versionを同TXで読むだけでは
古いZIP順のalignment/ページ帯を正しくできないと判明した。親・独立coreは次のprivate正規化を承認した。

- stored order versionが現行と異なる場合（旧/将来versionとも）、Complete ZIPだけを対象にする。
- 同TXの全item_id/item_keyを取得し、既存renumberと同じprefix除去・compare_book_pagesで採番する。
  hash/quality/index Noneを落とす前の全key集合を使い、その後に照会適格行へordinalを対応付ける。
  旧hash行が間にある実index0,2を、filter後の0,1へ詰めない。
- writer修復とreader private採番で全key SQL・純order helperを共有し、comparatorは引数で渡す。
  DBからscheduler/UIへ依存させない。mapは必要な対象ZIPごとにTX内共有し、全corpusの文字列表を常駐化しない。
- origin/candidateのVec自体をeffective ordinal順へ並べ直し、resolve_pages_by_item_id由来の辺にも
  同じmapを適用する。alignmentだけ新順、origin_page_keys/stripや対応辺だけ旧順という混在を作らない。
- DBのpage_index/version/item_changeを変更しない。現行versionはstored ordinal、PDF/physical順は従来どおり。
  将来versionを永続的に下げず、実行中viewerの現行順だけをprivateに導く。
- metadata読取失敗はquery Failedへ伝播する。恒久Preparing・旧順のReady・queryからの永続修復は行わない。

ZIP移動先はitem keyのentry名、帯clickはother_targetを使うので、private ordinalで別entryへ移動しない。
writer修復成功commitの通知（0containersを含む）は引き続き行い、失敗を成功通知へ変えない。
修復前後の結果/strip一致、旧hashの穴、quality0、index Noneと正しいentryへの移動先を回帰で確認する。
## worker 専有の MIH と世代選択

PDQ 256bit を16bit×16に分割する。1 block は距離2以内（137 bucket）、残り15 block は距離1以内
（各17 bucket）、計392 bucket を読む。どこにも入らない署名は距離が最低33なので半径32を漏らさない。
最後に全256bitの距離を検証し、row を重複排除する。これは近似・sampling ではない。

- `DerivedBookSearch` 相当が不変 `SearchSnapshot` と base / delta の row index postings を所有する。
- PDQ record 全体を別コピーせず、base の共有 owner へ MIH の `OnceLock` 等を加えない。
- quality>0 の base を索引化し、superseded mask は quality0・削除を含む全置換へ適用する。
  delta は最終 Live かつ quality>0 だけを索引化する。
- seen marks と scratch は worker 専有。待機 client・UI は snapshot や巨大配列を保持しない。
- cache は派生検索1世代。旧 MIH を drop してから新世代を構築し、他機構が保持する base Arc も peak RAM に数える。
- 同 store で `base_seq <= snapshot_seq <= TX_seq` の候補から `(base_seq, snapshot_seq)` 降順で選び、
  tie は既存 MIH を再利用する。base の Arc identity も照合する。
- private base0/snapshot200 と shared base100/snapshot180、TX201なら shared を選んで追随する。
  未来の配列を巻き戻さない。履歴不足時に作った同 TX の memory base は再利用できる。
- 取消された未完成 MIH は破棄。完成 base MIH は origin / scope 変更後にも再利用できるが、結果は再利用しない。
  store 変更・shutdown は MIH も退役させる。
- scope と本の有効性は同 TX の解決で検査し、postings 自体へ scope を焼き付けない。

## common と候補集合

common は半径32内の **有効な異なる9冊**を同 TX で確認した場合だけ成立する。起点自身の冊数も含む。
起点と候補の全ページを対象にし、同署名の判定は TX 内で memo 化する。

候補発見は既存の意味を保ち、**non-common な起点ページからの近傍辺が3本以上**の本を選ぶ。
異なる起点ページが3枚という条件へ変えない。1起点×3候補、3起点×1候補の両方を残す。
各起点で hit row を重複排除し、本ごとの辺数は発見段階のみ3で飽和できる。
発見時には候補側 common で辺を落とさない。候補 common の確定後、従来の反復署名の多重度を扱う。
alignment では厳密な辺を再列挙できるようにし、発見用の飽和を流用しない。

## 密な alignment のメモリ境界

以下は正当性を検討した設計候補で、製品実装・oracle 一致・処理時間と TX 寿命・採用は未検証である。

旧 Fenwick の tie と A/B 向き、実 page index、距離、出力順を保つ。
例: X 対 XXX は A0-B2、X 対 XXXX は A0-B0。最短対角を任意に選ぶ実装へ変えない。

O(N) の対角 shortcut は、eligible な両列が同じ長さで、実 index が順序付き一意、各k番目同士が
半径32以内の完全な順序付き全単射に限る。長さ違い・欠落があれば通常経路へ戻す。
shortcut でも params・同一本・署名幅・実 index 重複の入力検査を省略せず、N=0、最低一致数、
coverage、Strong / Weak の分類は既存の計算を共有する。

密な通常経路は全 E 辺や全 parent を保持しない。A group ごとの Fenwick score / stable tail を
block 境界で checkpoint し、必要な block を replay して復元する。
global B 圧縮は実際に辺へ参加する B のみ。`(a,b,d)` 順、重複の最小距離、A group の全 query 後に
B 昇順 update、従来 tie を保つ。A/B を転置しない。
復元時の A は厳密に減るため、各 block の再生は高々1回とする。
目標メモリは `O(ceil(N/K)M + KM + N + M + output)`、辺は概ね2回走査＋checkpoint copy
（B集合発見は別）。呼出元で全 E の Vec を作ったままにしない。
最大1万×1万の同一署名は1億の真の辺になるため、正確性を維持したまま実測する。

### 独立レビューで補足したcheckpointの条件

checkpointはglobal圧縮Bの全Fenwick cellを、A groupの開始前に保存する。
cellはmatched・u64 distance_sum・stable tailを含み、tailはglobalの実(a,b)で識別する。
replay内の一時Vec offsetをblock外へ残さない。Kはpage_indexの数値幅でなく行group数で切る。
既存score比較はmatched最大→distance_sum最小だけで、tail IDを比較しない。完全同点を決める
update順とqueryのcell訪問順を維持する。同TXの(a,b,d)整列・同(a,b)最小距離への一意化を
決定的に再生成すればstable tailから前blockのnode/parentを復元できる。
小さいoracleにX対XXX/XXXX、辺無しB座標を挟む圧縮、距離優先、K境界を跨ぐparentを含める。
全E保持の除去はclassifierのcandidatesだけでなく、PreparedCorpus.near_pairsや各Aの全近傍memoにも適用する。

1万×1万、K約100ならcheckpoint約100万cellとblock約100万parentになる。
entryが24〜32bytesの場合の両者合計48〜64MBは設計上の概算であり、実layout・一時allocation・MIH/base・
候補行と全hitのページ帯を含むpeak RAMの実測値ではない。

同長の全同署名だけでは対角shortcutへ入り、一般dense経路の検証にならない。
pair-levelの追加反例はA=X×9999,Y、B=Y,X×9999、dist(X,Y)>32（quality等の適格条件を満たす）。
対角shortcutを使えず約1億辺がある一方、最大9999一致の解は(i,i+1), i=0..9998に一意である。
巨大な旧二乗oracleを実行せず、一般dense経路の厳密期待値・時間・memoryを検証できる。
### 多候補の結果容量とページ帯consumer

独立UI監査で、全Eを除去しても現BookRelationHit.pagesの候補数C×起点全長Nが残ることを確認した。
独立した各起点署名に7別本×3同署名ページを対応させると、起点込み8冊なのでcommonでなく、候補発見の3辺を満たす。
N署名で7N候補が成立する。各pairはmatched1/Unrelatedでもqueryは全Ok結果を保持し、UIは部分的な重なりとして表示する。
候補truncateでは解決しない。採用前に以下の容量境界を実装・oracle・実測で確認する（型の具体実装は後続設計）。

- effective ordinal順のorigin page keyとExcluded/Unmatched基底をimmutable Arcで1つ共有する。
- hitはorigin slot順の一致overrideだけを保持し、Strong/Weak、対応page/target/item key/mtime/sizeを含める。
- strip.len()は常に起点全長。target解決失敗でもStrong/Weakの一致overrideは残す。
- 結果容量をO(N+C+Σalignment)にする。build_page_stripで基底を候補ごとに作らない。

現summarize_page_stripは全hitで起点全長相当をfoldし、book行には可視判定/virtualizationがない。
strip領域確保後にclip判定して画面外の集計/paintを省略し、全候補・scroll領域・divider・見える操作を維持する。
同じ描画passで列幅ごとの基底集計を共有し、overrideから列の色rank最大と最初のtargetを合成する案を検証する。
色rankはStrong>Weak>Unmatched>Excluded。click代表は最強のmatchでなく、その列で最初にtargetを持つorigin slot。
列境界のstart=floor(cN/W), end=max(floor((c+1)N/W),start+1)を共有し、単純floor(slot*W/N)へ変えない。
N=5,W=3,slot1で所属が変わる反例がある。[移動]はorigin順の最初のtarget、▼は起点全長に対する位置を維持する。

小oracleはN=8、各署名7冊×3ページの56候補、各strip長8/alignment1、base8+override56。
旧dense表現との全field等価、長短の帯、列境界、Weakが先/Strongが後、targetなし一致、Excluded、移動/▼を確認する。
画面外にも全候補分の領域を保ち、集計回数が可視stripだけになる回帰を加える。
現snapshot fixtureでは同一起点の先頭4ページが候補によりExcluded/Unmatchedと矛盾するため、一貫したfixtureへ直し
意図したPNG差分として独立UIレビューを受ける。

これは容量/consumerの設計監査であり、分類器全体のpeak・最終UI性能を達成した証明ではない。
O(C)のlabel/行処理も残るので、多hitの実UI計測と合わせて採用判断する。
## 正確性 oracle と採用前の確認

旧256 hit打切り照会や、候補列を `zip` するだけの比較は oracle にしない。
独立した brute-force の同 TX common / 全候補 key 集合と突き合わせ、relation / matched / distinctive /
coverage / alignment / strip を起点・候補 key で対応付ける。

実本の pair certificate は A/B の全ページ（quality0と実 index も含む）と、true common を示す
各9冊の実 witness を含める。false common は corpus の部分集合で増えない。witness を再帰的に展開しない。
通常・短い実本で旧分類器と全 field を照合し、密な例は小さい入力で exhaustive oracle を使う。
最大1万ページの certificate を旧二乗計算へそのまま流さず、構造化した期待値・証明と性能計測を使う。

必須の回帰・計測:

- 非推移性32/32/64、2冊300ページ、quality0置換、削除・delta・境界32/33、全候補 key と全分類 field。
- 同 TX 中の更新・第三本追加、OFF後prune待ち、配列遅延・履歴欠落・store違い、ページ順修復。
- A/B交互poll・同一本2client・park中完了・別client drop・連続refresh・取消/失敗後の自動次dispatch。
- cold / warm p50・p95、総CPU、**peak RAM**、base/delta/65535件compactionと他owner保持分。
- 実400ページ、短い本、最大1万、no-hit、多hit、near-white、反復署名。既存400ページ/7候補の
  元ケースが特定できれば同条件も測る。1.69秒は目標であり、未測定の合格値にしない。
- 最終 `scripts/test-full.ps1` と関連検査後に portable を作成し、利用者に従来と同条件で音声途切れを確認してもらう。

試作v2は単署名 kernel の厳密集合と限定計測まで成功している。本照会全体、世代整合、公平性、peak RAM、
最大密度の alignment、音声を含む採用条件の成功とは扱わない。
試作と検証用入力の証跡は `target/review-fixes-bench-20260908/`、現合意の根拠・訂正履歴は元の修正記録を参照する。
