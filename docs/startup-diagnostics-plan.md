# 起動診断と起動オーバーレイの有界化 — plan A

2026-10-06 / Phase 1 **設計のみ・未実装**。`58967bb9c` への再レビュー
`target/rplanA2-review.txt`（watchdogの所有/寿命にP2×2、他は受入れ）と、
設計ownerの「診断の監視対象を絞る」決定を反映した改訂案。
P1-1 は **2026-10-06 利用者決定: 旧Tantivyタグ一括移行を撤去**（§5.3）。
他のレビュー修正は維持する。本改訂を全体の独立レビュー承認済み・実装許可済みとは扱わない。
利用者指定: 次回 Microsoft Store 再申請に向けた plan A。
作業枝 `next-startup-diagnostics`、基準は master **`8a314a25f6df4c612b381f67a47cf39a8f8db74c`**。
本文の `file:line` は、この固定 master の行番号である。調査メモの v4.3.0 行番号を転記したものではない。
§1.335 の別枝を参照する箇所だけは明示的に別基準とする。

## 1. 観測、不変条件、保証する範囲

Store の報告は「loads indefinitely at launch」。こちらにスクリーンショット・ログがなく、
画面未表示、停止した描画、回転し続ける起動表示を区別できない。
[backlog §1.241](next-release-backlog.md)
の ORT / VC runtime 修正と Sandbox 成功を、今回の原因確定と扱わない。
`target/msstore-investigation.txt` は v4.3.0 の独立静的調査で、GPU、展開・DB I/O、初期フォルダ、
Indexer 無期限待ちが仮説である。本設計は原因を断定せず、コードで確認できた所有・待機境界を直す。

守る条件:

1. 起動各段階の **begin を処理の前、end を戻った後**に常時公開する。診断のディスク・mutex 待ちは起動側へ戻さない。
2. UI が update / present できる環境では、Indexer の生存したままの停止を全画面起動表示の無期限継続理由にしない。
3. オーバーレイ解除は検索準備の取消・失敗・完了を意味しない。同じ worker、同じ receiver、同じ writer を保ち、結果を一度だけ採用する。
4. 初期フォルダの解決・stat・走査は通常 UI の root present 後に worker で行う。起動場所・カーソル・明示ファイル open の意味を変えない。
5. 検索準備中の普通の閲覧・設定変更を許す。未準備サービスへの操作を「使えません」へ誤分類せず、遅い結果で新しい設定・navigation を上書きしない。
   旧タグ移行は利用者決定で撤去し、タグ操作に起動専用の制限・queue・移行準備状態を追加しない（§5.3）。

**保証の限界:** 「never stuck on startup overlay」は、本変更が扱う Indexer 待ちと初期フォルダ境界についての保証。
UI thread 自体が native call / 既存同期 I/O / 共通 logger 内で停止した場合、期限判定の実行も描画もできない。
画面前の settings / App DB / EPUB gate / asset 展開の背景化、共通 logger 全体の改修は今回行わない。
その停止は timeline と独立 watchdog が指すが、すべての環境で通常画面へ到達する保証とは別である。
**wgpu / D3D ドライバー内の停止解消は明示的に範囲外**。D3D11 の非同期化も今回は診断対象に留める。

`first_setup.rs`、初回設定の寸法、リリース版番号・署名・Store 提出は変更しない。
診断の自動送信、ログ送付 UI、起動失敗後の backend 再試行も範囲外。

## 2. 現行コードの違反点と所有者

| 対象 | master の経路 | 現行の問題 | 変更後の単一 owner |
| --- | --- | --- | --- |
| timeline | launcher `crates/launcher/src/main.rs:112` → `:119` → `:163` → `:192`、core `src/main.rs:3` → `src/lib.rs:947` | launcher 記録なし。core `src/lib.rs:390` の `emit_startup` は perf opt-in かつ主に処理終了時。初回 update を present と誤認し得る | 各 process に一つの `StartupTimeline`。lane の publisher はその実行 owner、一つの専用 writer がファイルを所有 |
| 表示 | `src/app.rs:27108` `render_startup_overlay`、`:84929` の表示・return | `startup_progress` の文字列 mutex のみ。段階開始時刻と総経過がない | timeline の Indexer lane から作る読み取り専用表示モデル。renderer は状態を書かない |
| Indexer init | `src/app.rs:3951` `StartupInitPending`、`:26214` kick、`:26275` 付近の spawn error 同期 fallback、`:26926` poll、`:84929` gate | `Empty` に上限なし。`startup_done` が poll を止める。manager / pending / done が状態を分割 | App-global `IndexerInit`（§5）。採用関数一つ、manager の owner もその enum |
| 初期場所 | `src/app.rs:85057` initialized 分岐 → `src/app/startup_ops.rs:1311` → `src/known_folders.rs:118` → `src/app.rs:23808` / `:25592` | Desktop / Previous / Specific 解決と実フォルダ scan が UI 同期。最後の overlay が画面に残る | 既存 startup / activation resolver の typed request owner を拡張。main navigation が採用し、scan 専用の別 startup owner は作らない |
| 明示起動パス | `src/app/startup_ops.rs:121` → `:316` → `:874` → `:1010` 前後の loader | 解決は worker だが Directory 適用後に loader の同期 scan に落ちる | 同じ resolver request が completed scan とファイル選択 continuation を運ぶ |

既存 heartbeat watchdog は `src/lib.rs:532` の `install_ui_heartbeat_watchdog` と
`src/lib.rs:1446` の設置、`src/app.rs:84357` 以降の heartbeat に依存する。
spinner が回れば heartbeat は更新されるため、Indexer の停止検出には使えない。
既存 `--diag-startup-windows` (`src/startup_windows_diag.rs:145`) は opt-in の HWND 詳細診断として保つ。
同機能の `first_paint.call_returned` は skipped paint でも到達するので present の正本にしない。

## 3. 常時 startup timeline

### 3.1 記録契約と launcher → core の受渡し

共通の小さな診断部品を launcher / core / vendored egui-wgpu から使用する。
新規候補は `crates/startup-diagnostics/`（I/O publisher と writer、時計、固定 Stage ID）。
アプリ固有の Japanese 表示、target 選択、DB の挙動はこの crate へ移さない。
既存 perf logger / 通常 logger を呼ばず、既存 logger の mutex を共有しない。

各記録は schema version、製品 version、run ID、process role、PID / TID、lane、span ID / parent、
stage ID、begin / end / milestone / overdue、結果、QPC ticks / frequency、run 起点からの単調経過を持つ。
end は duration と成功 / error / skipped を含める。返らない処理に成功 end を捏造しない。
run ID は launcher の PID と entry QPC の組（直接 core 起動では core の組）で作り、同一 machine 起動を区別する。
wall clock は header の対応時刻だけで、期限・順序・経過は Windows の QPC を使う。
`Instant` の内部表現を process 間で渡さない。Windows の同じ QPC 起点と周波数を共有して launcher/core の時間差を測る。

launcher は `main` の最初、core は `src/main.rs:3` の最初にメモリ owner を作り、
runtime lease (`src/lib.rs:949` 前後) より先に entry を公開する。
worker / help / 二重起動経路も entry と terminal 理由を残し、GUI ready 待ちを開始しない。

**選択: launcher の小さい専用ログ + bounded env snapshot。** launcher は perf logger を導入せず、
自分の専用 writer に記録する。core の Command へ run ID / entry QPC / frequency と
spawn 直前までの固定長 journal を内部 env（仮称 `MIV_STARTUP_TRACE_V1`）で渡す。
env はサイズ上限 24 KiB、最大256件の固定binary eventを符号化し、schema・数値・件数を検証する。
上限時はentryを含むheaderと直近eventsを保持し、落ちた件数を添える。不正なら inherited trace を捨てて独立 run とする。
未知のファイルパスを env から開かない。core は snapshot を inherited launcher events として自身の writer に渡す。
launcher spawn end、child PID、runtime lease handoff (`crates/launcher/src/main.rs:194`–`:200`) は
spawn 後なので launcher ファイルだけに残る。同じ run ID で結び、snapshot と元ログの重複は span ID で区別できる。

env だけの案は core entry 前の停止に記録が残らないため不採用。
起動 thread が小ファイルを同期 write してから spawn する案も、展開と同じ I/O 停止を作るため不採用。
既存 runtime handoff を新しい present 監視・ACK 待ちへ変更しない。launcher-supervised fallback は §9 の別作業。

### 3.2 起動側は publish だけ、writer は独立

- lane は固定の少数（launcher main、core main/render、Indexer init、initial navigation、metadata orchestration）。
  実行 owner が begin/end を発行し、その lane の現在 stage と stage 開始単調時刻を atomic に公開する。
  子 span へ入る前後は owner-local な固定深さ stack で親を復帰し、親 total と子 duration を二重加算しない。
  supervisorごとのwatch登録は既存registration ID等を添えた個別timeline eventにし、
  並行supervisorが一つのcurrent-stage laneを上書きする形にしない。watchdog対象とは別（§3.5）。
- この current-child と local stack だけでは親 await を監視できない。§3.5 の固定 parent-watch slot を
  別に公開する。子 stage の開始・終了・親への復帰は、その slot の時計や通知済み threshold を変更しない。
- current stage の ID と開始時刻は一つの `AtomicU64` に pack し、watchdog/UI が異なる世代の ID と時刻を組み合わせない。
  例: 16bit ID + run-relative 48bit microseconds。長い寿命を前提にせず startup 用とし、同じ stage 再入にも開始時刻で identity を付ける。
  diagnostics snapshot のための spin loop、seqlock 再読ループ、`try_lock` + sleep を作らない。
- event は固定サイズの構造体。固定 journal / queue への登録は一度の `try_lock`、取れなければ atomic dropped count を増やす。
  mutex 下で QPC 以外の OS API、format、JSON、ファイル I/O をしない。current stage は登録失敗でも先に publish する。
  journal は 1024 件 / process を上限とする。過剰時は diagnostics を欠落させ、起動を待たせない。
- 初期navigationのtraceはrequest ID付きのhandleをそのrequestが所有する。取消時にtraceをterminalにして
  止まらない旧workerのlate eventで別requestのstageを上書きさせない。
  初期explicitと一度のdefault fallback以外の通常navigationへstartup laneを使い回さない。
  Indexerはprocess-wide、フォルダ要求はmain-context固有とし、ログの相関もこの境界を保つ。
  watchdogは初回dispatchだけ。Remote返却後の再dispatch/default fallbackはtimelineを残しても
  watch slotを再利用・新設しない。Remote取得時の監視退役は§3.5の限定契約による。
- writer は queue から固定 batch を取り、guard を解放して JSONL encode / directory 作成 / write / flush を行う。
  begin、end、watchdog event は短い buffer のまま放置せず writer 側で flush。`sync_all` は要求しない。
  disk が止まっても publisher/watchdog/UI は進める。watchdog と writer は別 thread とする。
- launcher の handoff snapshot は journal の一度の非待機取得で作る。競合なら欠落数を付ける。
  launcher exit に writer drain の待機を追加しない。末尾の未出力はあり得るが、core に渡った pre-spawn snapshot で補う。
  core も終了時に writer の無期限 join / flush をしない。diagnostics の shutdown は best effort。
- writer / watchdog の spawn error、disk-full / permission error は diagnostic-unavailable をメモリへ公開して終わる。
  通常 logger へ再入して報告しない。起動後に通常 UI が通知可能なら一度だけ「起動記録を保存できませんでした」と表示する。
  一度限りの通知はoverlay中・トレイ非表示中・OS最小化中には消費せず、通常UIが表示可能になった時点でtoastの期限を開始する。
  自動 retry、別 disk への多段 fallback、設定ファイルの削除は作らない。

通常版の保存先候補は `%LOCALAPPDATA%\mimageviewer\startup-logs\<run>-<role>-<pid>.jsonl`。
network/redirected APPDATA の停止と保存先を分ける。portable / 明示 `--data-dir` / headless test は
その disposable data 配下 `logs/startup` に保存し、通常 profile へ書かない。
launcher も引数に明示 data-dir がある場合は同じ方針を採る。log path 自体も header に記録する。
LOCALAPPDATA 自体が redirected / 不可の場合の永続記録は保証しない。disk と OS の両方が止まる場合まで
記録を必ず残す保証には、別 process / 別 sink が必要で、今回は作らない。
ログはローカルのみ。起動時 target / data path を含むので、共有時には利用者が内容を確認する。
writer による診断ログだけの容量制限は 1 file 2 MiB / 最近 10 run を提案。
保持数整理は新ログの記録後に行い、active writer のファイルは即時ロック確認で skip（待たない）。
削除失敗は記録して終了し、設定・索引を掃除する処理とは共用しない。

### 3.3 data_dir と環境

`src/data_dir.rs:25` `init` の begin/end を記録し、決定後に **実際の data_dir**、選択理由
（normal APPDATA / portable / explicit）、APPDATA、LOCALAPPDATA、runtime extraction path を添える。
文字列の UNC（`\\server\share` と `\\?\UNC\...`）判定は純粋処理。
既存 `src/data_dir.rs:175` の mapped-drive 判定は `GetDriveTypeW` を呼ぶため publisher/UI で呼ばない。
drive type、known-folder の通常位置との差、reparse / resolved location は短命 environment worker が調べ、
照会自体にも begin/end を付ける。UI / writer / watchdog は照会完了を待たない。

header / 補足には `network = yes/no/unknown`、`unc = yes/no`、
`redirected = yes/no/unknown` と根拠（env 指定先、Shell 通常位置との差、reparse target 等）を残す。
APPDATA の文字列差だけで folder redirection が無いと断定せず、未照会・照会停止は unknown。
祖先のreparse pointもenvironment workerで確認する。local driveでも祖先にlink/reparseがある、
または祖先情報が読めない場合、到達先のlocal確認をしていないためnetworkはunknown。
`C:\link\data`の`C:\link`がUNCを指す場合もnoとしない。literal UNC/remote driveはyesを維持する。
診断のためのlink target解決・再試行は加えず、ログにtarget未照会を明示する。
data_dir の canonicalize / ネットワーク接続の確認を診断目的で起動 thread に追加しない。
GPU 情報は既存 adapter 情報から backend / name / vendor / device / driver 情報を記録し、
診断のために別 GPU device を作らない。driver 欄が API から得られない場合は unknown。
root HWND visible / minimized / position / DPI は既存確定 root handle から一度記録する。
HWND の geometry 推測や全 detached window 列挙を常時 timeline に持ち込まない。

### 3.4 必須 stage の配置（begin/end 両方）

表の参照位置は固定masterの現行経路。§5.3で撤去する旧タグ移行には新しいtimeline stageを実装しない。
現役のApp tags.db open、通常タグwriterと他のIndexer段階は維持する。

| stage 群 | master の境界 | publisher（単一 owner） |
| --- | --- | --- |
| launcher entry、引数、既存 instance / IPC、runtime path | `crates/launcher/src/main.rs:112`, `:119`, `:133`, `:137`, `:492` | launcher main |
| runtime parent mkdir / canonicalize、in-use lease、extraction lock、version dir | 同 `:139`–`:161` | launcher main。lock の既存 60 秒上限を変えず、前後 I/O と区別 |
| 各埋込 asset の確認 / hash / temp write / rename | 同 `:206`, `:251`, `:364` `ensure_asset` | launcher main。asset name / byte size / reused or extracted、各 duration |
| EffeTune inventory / generation reuse / publish lock / 展開 / hash | 同 `:166` → `crates/launcher/src/effetune_bundle.rs:39`, `:73`, `:120`, `:167`, `:202`, `:261` | launcher main。optional error と返らない処理を分ける |
| core spawn、runtime lease handoff、launcher terminal | `crates/launcher/src/main.rs:192`, `:196`, `:204` | launcher main |
| core entry、runtime pin、worker mode / help 判定、data-dir、instance / IPC | `src/main.rs:3`, `src/lib.rs:947`, `:949`, `:999`, `:1041`, `:1073` | core main |
| logger init、EffeTune pin、EPUB gate | `src/lib.rs:1100`, `:1105`, `:1113` → `src/epub_cache.rs:1561`, `:1585`, `:1608` 付近、lock `:1529` | core main。logger 記録より先に timeline を publish |
| AI model / Susie worker 展開、icon | `src/lib.rs:1198`, `:1204`, `:1290` | core main。モデル別・Susie comparison/write も分ける |
| settings load 全体 / DB family 判定 / open / pragma / integrity / schema / JSON migration / recovery / keymap | `src/lib.rs:1212` → `src/settings.rs:8818` → `src/settings_db.rs:3564`, `:551`, `:598`, `:3468` | core main。既存 finite retry の試行番号を span に添える。設定未読の defaults 保存禁止を維持 |
| Collection actor、RAW executor、Remote の同期 reader / service 準備 | `src/lib.rs:1373`, `:1389`, `:1398`, `:1409` 前後 | core main。spawn と ready / background serve を区別 |
| run_native / window create、wgpu Instance | `src/lib.rs:1449` → `vendor/eframe/src/native/wgpu_integration.rs:286`, window builder `:1169`、`vendor/egui-wgpu/src/winit.rs:100` | core main / eframe renderer |
| surface create、adapter enumeration / selection、device request、surface capabilities、pipeline、configure | `vendor/egui-wgpu/src/winit.rs:190`, `:238`, `:314`, `:156`、`vendor/egui-wgpu/src/lib.rs:192`, `:209`, `:225`, `:240`, `:248` | egui-wgpu initialization owner。個別 begin を native call 前に公開 |
| creator entry、fonts / theme、App constructor | `src/lib.rs:1455`, `:1465` 前後, `:1471` 前後, `:1485` | core main |
| D3D11 video device / video interface / fence / shared handle | `src/lib.rs:1545` → `src/video/gpu_renderer/d3d11_device.rs:603`, `:616`, `:635` 前後, `:661`, `:668` | core creator。動画を開かなくても実行される現行を明示 |
| favorite adjustment / view-state hydration、名前索引 worker spawn、creator exit | `src/lib.rs:1563`, `:1564`, `:1570` 前後, `:1591` | core creator |
| first update begin/end | `src/app.rs:87296` 外側 update の入口/末尾 | App root update。内側 early return でも end、panic 時は end なし |
| first render：texture delivery / buffers / acquire / encode / submit / present | `vendor/eframe/src/native/wgpu_integration.rs:877` → `vendor/egui-wgpu/src/winit.rs:479`, `:511`, `:545`, `:558`, `:654`, `:668`, `:693` | root painter。§3.6 の成功境界を使う |
| Indexer init、各子段階、結果送信 / UI 採用 | `src/app.rs:26214`, `:26251` 前後, `:26926`、下表 | startup-init worker / App の採用関数 |
| initial target resolution / scan / async enumeration / adoption | `src/app/startup_ops.rs:121`, `:316`, `:1311`、`src/app.rs:25655` | startup request worker / main navigation。request ID 付き |
| normal UI draw / first normal root present / initial target terminal | 現 `src/app.rs:84929`, `:85057` と root paint 境界 | App は draw tag のみ、painter が present を確定。ready と target 完了を別 milestone にする |

App constructor は `src/app.rs:17342` 以降。`app_default` の一括 duration だけにせず、
以下の DB open と初期 read / migration の **各々**に stage を置く。同一 core creator が publisher。

| DB / load | master の入口 |
| --- | --- |
| archive cache / edit-preview service / search index | `src/app.rs:17350`, `:17357`, `:17370`（edit-preview 内部 cold open `src/edit_preview_cache.rs:1244`） |
| rotation open / keys、audio normalize、video pin、folder pin migration | `src/app.rs:17377`, `:17388`, `:17393`, `:17398` |
| video bookmarks、book bookmark service spawn、chapter / tile thumbs | `src/app.rs:17422`, `:17426`, `:17429`, `:17433` |
| rating / tags / spread / view trim | `src/app.rs:17439`, `:17440`, `:17444`, `:17452` |
| book resume open / startup read、reading history、auto-aspect | `src/app.rs:17488`, `:17496`, `:17501`, `:17510` |
| PDF passwords / DPAPI、adjustment open / keys、local adjust open / keys | `src/app.rs:17524`, `:17528`, `:17539`, `:17545` |
| crop / mask / conceal / comic open と各 keys、user stamp | `src/app.rs:17576`, `:17582`, `:17587`, `:17593`, `:17598`, `:17604`, `:17609`, `:17615`, `:17620` |
| upscale queue lock/load/save、keymap reference write、folder pane drives | `src/app.rs:17624`, `:17628`, `:17642`, `:17672`, `:18122` → `src/folder_pane.rs:134` → `src/known_folders.rs:256` |

Indexer 内部も「整理中」の一段へ丸めない。

| Indexer 子 stage | master の入口 | owner |
| --- | --- | --- |
| fts_meta presence / open / schema | `src/indexer_manager.rs:212`, `:214` | startup-init worker |
| rebuild marker read / old index wipe | `src/indexer_manager.rs:226`, `:244` | 同じ worker |
| Tantivy open / schema recreation / reader | `src/indexer_manager.rs:260` → `src/fts_index.rs:350` | 同じ worker。meta lock と writer lock を混同しない |
| inventory reset / marker complete | `src/indexer_manager.rs:274`, `:287` | 同じ worker |
| IndexWriter / dispatcher start | `src/indexer_manager.rs:422`, `:438` → `src/fts_index.rs:406` | 同じ worker、生成後の唯一の writer は既存 dispatcher |
| metadata runtime spawn / manager return | `src/indexer_manager.rs:446`, `:467` 前後 → `src/metadata_reconfiguration.rs:93` | startup-init worker |
| cleanup / reconciliation / supervisor spawn / orchestration idle | `src/metadata_reconfiguration.rs:271`, `:345`, `:349`, `:518`, `:533`, `:560` | 既存metadata worker。自身の作業begin/endだけを記録し、idleをwatch登録完了と呼ばない。timelineのみ、watchdog対象外 |
| supervisorごとのrecursive watch登録 begin/end（Ready/Unavailable/cancel） | `src/indexer_supervisor.rs:377` 前後、既存集約判定は `src/similar_index.rs:1880` | FsWatcher::startと既存registration terminalを扱う当該supervisor。利用可能な既存registration ID/favorite IDとspan IDで相関。timelineのみ、watchdogのparent/終端通知/新しい集約ownerは足さない |

timeline の観測を入れるために DB の recovery / 索引再構築仕様は変更しない。
背景 AI / WASAPI / Susie / VST / EffeTune / tray の開始と結果は補足として区別し、
それらが通常 UI の待ち条件であるような単一「起動中 stage」を上書きしない。

### 3.5 UI heartbeat と独立した watchdog — 監視経路の限定

**2026-10-06 設計owner決定（再レビューP2×2への対応）:** watchdogは診断であって正しさのownerではない。
Remote延期/返却/再dispatchやmetadata登録のライフサイクルを診断のために新しく集約せず、
既に単一ownerが明確な **launcher → core entry → settings → GPU/surface → first PRESENT →
Indexer init（採用/終端まで）→ 初期targetのFIRST dispatch → normal UI ready** の起動経路だけを見る。
この列挙は監視範囲であり、処理を直列化する指定ではない。§3.6どおりnormal UIのroot presentは
初期target dispatchより先でよく、Indexer Pendingはnormal UI ready後も同じ採用点の終端まで監視する。

launcher entry / core entry に専用watchdogを開始。250ms程度のtimed waitで公開メモリを読み、
同じ監視spanの **5/15/30秒を各一回**だけ非待機enqueueする。
heartbeat/spinner/短いchild完了で期限をresetしない。writer I/O/普通のloggerを呼ばず、
App状態、navigation、watcher readiness/barrierには作用しない。overlay期限とは別の診断である。

**公開方法:** current-childとは別の固定parent-watch slotを、
launcher handoff、core first-present、normal-root-present、await-Indexer-init、
initial-target-first-dispatchだけに限定する。metadata/fallback/再dispatch用slotは作らない。
各slotはrun/slot IDと元のbegin時刻を持ち、実行ownerだけがAtomicU64のwatch clockをpublishする。
clockはReserved（未開始）/Active（有効開始時刻）/Suspended（意図的待機までの有効経過）/Retired。
Reservedはresumeで開始せず、実dispatch/paintのbeginだけがActiveにする。診断時計のみの区別で、業務stateではない。
pause時はnow − effective_start、resume時はnow − saved_elapsedを使う。
watchdogは元identityごとの3bit通知maskを自分だけで所有する。childへの出入り/親復帰で
親時計や通知済みmaskをresetせず、parent beginを再発行しない。snapshot再読ループは作らない。
子identityの通知maskは各slot最大128件。129件目を観測した時点で
`watch.children.capacity_exceeded`（capacity=128 per slot、watch=parent-only）を一度記録し、
そのrunの以後は全slotで親spanのみ監視する。128件目までは監視し、既知childへの復帰も
超過後は再監視しない。table再利用・回復・再試行は加えない。親の5/15/30秒と終端条件は維持する。

初回dispatch前のslotはReservedで予約し、present完了→次updateのdispatch間で
全slotが一瞬退役してwatchdogが先に終了することを防ぐ。不要/取消/Remote取得はこの予約も退役する。
初回のwatch handleだけを当該dispatchへ渡し、slotは再利用しない。後続要求のtimeline handleには
watch handleを付けない。新しい業務dispatch count、Remote resume state、第2coordinatorは追加しない。

| watch対象 / 単一発行owner | 開始・終端 |
| --- | --- |
| launcher handoff / launcher main | entry → core spawn/lease handoff成功、起動失敗または終了。展開childが切替わっても総待ちを監視。core画面のlauncher監督はfollow-up |
| core first-present / core main → root presenterへの所有権受渡し | core entry → 最初のroot PRESENTが戻る、fatalまたは終了。settings/App構築/GPU/surfaceをこの経路のchildとして記録・監視。run_nativeのプロセス寿命は監視しない |
| root paint / 既存root presenter・normal-root-present owner | first PRESENTのacquire/submit/present呼出しは経路内のchild。normal-root-presentの親はoverlay解除後の通常paint要求 → NormalShellのpresent-return、fatal/取消/終了。利用者hide/OS minimizeは既存ownerの事実で意図的待機とし、restoreで同じ画面待ちを再開。白フラッシュ対策のbootstrap非表示は利用者hideと区別 |
| await-Indexer-init / App-global初期化owner | worker dispatch → 単一採用点のReady/Unavailable（spawn失敗/disconnect含む）または終了。normalready/tray hideで実初期化の監視を止めない |
| initial-target-first-dispatch / 初期navigation request owner | 初回のresolver/scan dispatch → 当該結果の採用/失敗/取消、Remote取得または終了。初回がdefault targetでも同じ。modal admission前は未dispatch予約であり停止扱いしない。Remote以外の既存modal-held待ちは意図的待機として扱う |

**Remote取得の限定契約:** startup_ops.rs:698の既存取得/取消ownerが初回dispatchのwatch spanを
**suspended (not watched)** と記録して監視を終了する。これは再開可能なwatch clockのSuspendedではなく、
診断上のterminal outcomeであり、slotはRetiredにする。初回dispatch前に取得された場合も予約を退役する。
論理argvを保持して返却後に再dispatchする既存仕様（startup_ops.rs:32）はそのまま維持するが、
返却/再dispatch/繰返し取得でwatchdogを再起動・slot再利用・新slot作成しない。
旧workerのlate eventで退役slotを復活させない。Explicit→Defaultの一回fallbackも初回の次ならtimelineのみ。

**監視しないが記録するもの:** Remote返却後の再dispatch、fallback等の後続要求と、
metadata watch-bootstrapの各registrationはrequest/registration ID付きtimeline begin/endを残す。
再dispatchのresolver/scanは当該request owner、FsWatcher::startの実登録とReady/Unavailable/cancelは
当該supervisor（indexer_supervisor.rs:377）が記録する。reconfiguration workerのspawn/idle
（metadata_reconfiguration.rs:533/:560）は自身の作業終端であり、実watch登録完了の代理ではない。
similar_index.rs:1880の既存readiness集約には、新しい診断終端通知/parent slot/集約ownerを足さない。
登録が止まればそのregistrationのbeginにendが無いことからログで境界を追えるが、
5/15/30秒のoverdue通知、watchdog延命、再起動は保証しない。product側のbarrierは変更しない。

watchdogのchild allowlistも上の起動経路と初回dispatchの有効watch handleだけに限定する。
timeline laneの全current-stageを走査して、対象外の再dispatch/metadataを間接的に監視してはならない。
GPU停止では最後のadapter/device/D3D11/present beginと対象経路のoverdueが境界を示すが、停止解除はscope外。
run_native常駐、通常フレーム、modal/Remote操作待ち、background supervisor寿命は起動停止扱いしない。

**終了条件:** launcher watchdogは自身のhandoff終端、core watchdogは自身の上記slotが全て
Retired（NotApplicable/Remoteのsuspended-not-watchedを含むterminal outcome）になった時、
またはprocess shutdownで終わる。
normalreadyだけではIndexer Pendingや初回dispatchの予約/実作業を退役しない。
一方、監視経路が終端ならRemoteの保留/再dispatchや未完了metadata登録が残っていても終了する。
timeline writer/event publisherは必要なbegin/end記録を継続し、watchdog寿命とは連動させない。
終了時にUIからwatchdog/writerを無期限joinせず、30秒以降の連打/親復帰時の再通知もしない。

### 3.6 first PRESENT と通常 UI ready の正本

`src/app.rs:84870` の既存 `first_frame` は first-update として保つが、present と呼ばない。
新しい証拠は **root の surface acquire 成功 → queue submit → `output_frame.present()` が戻った直後**
（`vendor/egui-wgpu/src/winit.rs:693`）。API は画面の実 pixel 可視や DWM 完了を保証しないので、
ログ契約は `first_present.returned` とする。API 内の hang は begin だけになる。
surface absent / timeout / lost / zero-size / paint skip / submit のみでは first-present を確定しない。

vendored `WgpuConfiguration` に任意の軽量 diagnostics callback を渡し、初期化 span と
root paint outcome を固定 event として core の timeline へ発行する。callback は軽量 publish と、
§6.1 の生きた Deferred 要求に登録された root wake だけを呼ぶ。I/O・App の採用・resolver 実行はしない。
App は root の該当 frame の draw が Overlay / NormalShell のどちらかを小さい frame tag に公開する。
eframe が root update の full output を paint へ渡す時にその tag を捕捉し、同じ paint attempt に対応させる。
egui の複数 pass / discard の場合は採用された最終 pass の tag を使い、前 pass の Normal tag を流用しない。
root と frame identity を一緒に渡し、deferred / immediate detached paint から root-ready を立てない。

最初に Overlay を present すれば `first_present.returned` のみ。
NormalShell tag の root present が戻った時に `normal_ui.ready` を一度 publish する。
App は次の root update でその atomic milestone を読む。**この後にだけ初期 target request を dispatch**する。
その「次の update」を偶然の入力や Indexer poll に任せない。Deferred owner の一回の present-completion
wake と、normal paint 未成功中の所有タイマー（§6.1）が進行を保証する。
通常 UI ready は「shell が描けた」であって、検索利用可 / 初期一覧の読込完了ではない。
初期 target の一覧採用、対象ファイル/本の表示、通常 UI の first present は別の相関イベントを持つ。
初回設定・更新通知など既存 modal が NormalShell 上にある場合は modal を隠さず、navigation admission を待つ。

## 4. 起動オーバーレイの表示

owner は timeline の Indexer lane。現在の `startup_progress: Arc<Mutex<String>>` と
`make_progress_hook` を stage publisher へ置き換え、文字列と時計の二重 owner を残さない。
stage → Japanese は固定対応表で、UI 側の renderer が時計を読み出すだけにする。

| 内部段階 | 表示例 |
| --- | --- |
| meta DB / schema | 検索用データを開いています |
| wipe / recreation | 検索用データを作り直しています |
| Tantivy index / reader | 検索の準備をしています |
| writer / runtime spawn | 検索の準備を仕上げています |

中央表示例: 「起動中…」「検索用データを開いています」
「この処理 2秒 / 起動から 4秒」。stage ID、Tantivy、writer、adapter、reconciliation は表示しない。
秒は単調時計から求め、段階切替で「この処理」だけ reset。総経過は launcher entry（直接 core は core entry）。
0秒を許し、長い値でも折返して小窓に収まる。偽の百分率・残り時間は出さない。
UI が描ける以前の launcher / settings / GPU stage はログ対象であり、overlay を表示できたと偽らない。
Overlay 表示中は現行と同じ入力消費・×終了を保ち、解除 frame で蓄積 input を一度消費する。
通常 UI へ移った後は起動理由の全体 input 消費を続けない。

通常 UI で init が Pending の間は持続表示「検索の準備中」（必要なら stage / elapsed の hover）。
global / item search 等、manager を必要とする操作には同じ状態を示す。通常のフォルダ閲覧をブロックしない。
Unavailable は「検索を準備できませんでした。再起動すると再度準備します」を一度通知し、
既存 Failed の具体的な user_message は失わない。名前索引など独立サービスまで不可扱いにしない。

## 5. Indexer 初期化の有限 overlay と一度だけの採用

### 5.1 最小の状態と期限

**N = 5 秒を提案**。startup-init worker の開始時刻から5秒で full-screen overlay を解除する。
起動前の展開/GPU時間をこの5秒に含めず、その長さは timeline の total に見せる。
5秒は表示上限であり init 処理の実行 timeout ではない。deadline 到達時に結果があれば結果採用を先に行う。
Ready / Unavailable / Failed が早く届けば5秒を待たず解除。

型の概形:

```rust
enum IndexerInit {
    NotStarted,
    Pending { rx: Receiver<StartupInitOutcome>, started_at: Instant,
              full_check_requested: bool },
    Ready(IndexerManager),
    Unavailable { message: String },
}
```

App-global のこれ一つで現在の manager / `startup_init` / `startup_done` の状態分割を置換する。
Ready manager への accessor で既存 consumer を移行。`full_check_requested` は既存の単一集約を保持する。
`TimedOut` / `DetachedInit` / 再試行 pending / 第2 manager は足さない。
`overlay_visible(now)` は NotStarted または Pending の elapsed < 5秒から導出する。
期限が過ぎた Pending は同じ Pending で、receiver を drop しない。
`ui_unblocked`、`indexer_terminal`、`indexer_available` は別の **導出述語**であり別 bool にしない。
`normal_ui.ready` は描画の実 milestone (§3.6) で、Indexer 状態を複製しない。

### 5.2 poll と採用点

root の外側 `App::update` (`src/app.rs:87296`) で、`update_frame` より前に一度
`start_if_not_started` / `poll_indexer_init` を実行する。overlay、動画、fullscreen の early return でも poll が消えない。
終端 worker result、spawn failure、channel disconnect は同じ `adopt_indexer_init_outcome` に入る。
この関数だけが Pending → Ready / Unavailable を遷移させ、manager を install する。
初期化中だけ ROOT を明示した 100–200ms の request_repaint_after_for と残り overlay deadline の短い方を
毎 pass 要求する。hidden/minimized も既存 scheduler の requested work として poll を進める
（vendor/eframe/src/native/run.rs:558、idle hidden を常時起こす変更はしない）。
通知のための第2 poll、writer 起動、timeout thread からの App 書換えは作らない。

採用時の順序:

1. Pending を `take` 相当で退役し、同じ outcome を二度採用できなくする。待ち時間と result を timeline に publish。
2. Ready manager を露出/設定提出する前に、**採用時点の現在の tray/visibility** に
   set_io_throttled(!window_visible)（src/indexer_manager.rs:664）で合わせる。
   Ready manager を enum に移し、**採用時点の最新** favorites / excluded roots / PDF passwords /
   similar configuration を既存 `sync_with_configuration_and_passwords` などへ提出する。
   init の古い snapshot で UI 設定を巻き戻さない。speed は現在も次回起動反映
   (`src/ui_dialogs/preferences/pages.rs:7438` 前後) なので、この起動の初期 snapshot を維持する。
   存在しない live speed setter を追加しない。`skip_offline_change_scan` も既存の初回構成契約を保つ。
3. Pending 中に集約した明示 full check は、Ready 後に同じ manager へ一度要求。Unavailable なら準備できない旨を一度通知。
4. housekeeping、独立 similar の watch bootstrap、names sync は既存 terminal 契約へ進める。
   UI の解除 deadline では watch bootstrap を閉じない。

`src/app.rs:26123` の `refresh_similar_index_password_config` は現在 `startup_done` を
「manager なしが終端」の代理に使う。Pending で通常 UI を出しただけではこの枝へ入らないよう、
enum の Unavailable を明示して扱う。`request_index_full_check_shared` (`:26071`) は Pending 内へ集約を続ける。
`poll_housekeeping_arm` (`:26893`) は検索終端 / supervisor idle 条件を維持。
update check / runtime cleanup (`:27012`, `:87386`) は normal UI ready に依存し、Indexer 利用可とは区別する。
VST / EffeTune polling の `startup_done` gate (`:84950`) も UI 開始と検索終端を混同しない。
global search (`src/global_search_ui.rs:2280`)、索引 status、favorite 操作、設定変更の全 consumer を棚卸しして移行する。

worker spawn 失敗は即座に Unavailable outcome を同じ採用点へ渡す。
`IndexerManager::new` を UI thread で再実行する現 fallback は削除。
disconnect / panic でも同じ terminal tail が similar watch bootstrap を閉じる。
通常の Unavailable は既存失敗結果を保ち、再起動まで自動 retry しない。
終了時は receiver を落とし、停止した初期化 thread を UI から join しない。
未採用 Ready manager は worker 側で既存 shutdown 所有規約に従って処分し、UI や sibling context へ移さない。
Ready 後の shutdown は既存 manager-wide bounded stop / writer finalizer の契約を維持する。

**表示状態の owner（P2-4）:** hide_to_tray（src/tray_integration.rs:278、throttle :321）と
sync_after_restore（:355、throttle :366）が正本。既存 update_frame の外部 ShowWindow 同期
（src/app.rs:84615 付近）を、当該 root pass の採用前に同じ helper 経由で反映する。
HWND capture/既存 tray-active gate を保ち、同じ可視状態の第2 bool や新しい OS polling owner を作らない。
閉じる要求を tray が受理しても既存 viewport 登録を省略する early return は加えない。
Pending 中の hide は manager が無いため現 :321 の throttle が空振りするが、上の採用点が補う。
ActivityGate は init に渡した共有 gate で、pause_indexer_while_minimized による既存 pause と
I/O semaphore の throttle は別。採用時に gate を resume/reset せず、最新の既存 gate 状態を使う。

OS minimize は SW_HIDE/tray退避と同じではない。window_visible=true の単なる最小化を
検索の新しい throttle/pause 条件にしない（名称だけから設定の意味を広げない）。
起動途中の tray hide はこの一回の late adoption、常駐インスタンスへの activation は既存
restore/activation routing であり、新しい startup worker/default folder/overlay を開始しない。
hide 後も Pending poll の所有要求は継続し、hidden scheduler 経由で結果を採用してから throttled に保つ。
restore は同じ manager を通常速度へ戻す。hidden quit は Pending receiver/request を退役し、
未採用結果を復活させず init worker を UI join しない。Ready は既存 bounded shutdown に従う。

### 5.3 旧Tantivyタグ一括移行 — 撤去決定（P1-1）

**2026-10-06 利用者決定: 起動時の一度きりの旧タグ移行を撤去する。**
本Phase 1は設計のみで、コード上の撤去は後続実装に含める。
移行と編集を順序付ける案、overlay延長、タグ操作の制限案は採用しない。
前改訂のタグ操作制限案、タグadmission/準備中notice/proof通知、専用の三状態、sidecar保留、
Remote tagの移行理由Busy、移行失敗のための旧索引保持/Blocked処理は設計から削除する。
同じ受信channelには終端StartupInitOutcomeだけを運び、新しいmigration eventを追加しない。

**理由と受け入れる損失:** この移行が必要になるのは主に、v1.0〜v1.3でタグを利用し、
v1.4.0以降を一度も起動せず、次のreleaseへ直接更新する利用者。
v1.4.0（2026-06-13）から約4か月・約20releaseを経たため、このまれな直更新の救済より
単純な仕様を優先する、という利用者判断である。人数/規模の計測結果に基づく推定ではない。
直接更新時、旧Tantivy STORED tagsの値は新たにtags.dbへ取り込まれず、
旧タグをmIVのタグ一覧・タグ検索で利用できるようになるという従来の救済を失う。
元ファイル/動画sidecarのXMPに残っている旧タグはそのまま残し、移行目的の読み取り・削除・書き換えをしない。
Tantivy索引自体は従来のrebuild/ingestで更新され得るが、タグ移行用に保存し続ける新機構は作らない。
手動legacy XMP取込はv3.4.0、自動seedはv3.9.1で撤去済みであり、復活させない。
「XMPにあるのでmIVが後で自動救済する」と説明しない。

既に移行済みのtags.db、通常のタグ付与/削除/改名/Undo/Redo、mIV sidecarのバックアップ/復元、
metadata import、タグ保存を伴うcopy/move/delete、smart folder、Remoteの独立タグserviceは維持する。
Indexer Pending/Unavailableを理由にタグ操作を止めない。タグDB/writer自身の既存Unavailableは別に扱う。
既存item_tags/tag_item_state、歴史的source='tantivy_migration'の行、既存tag_meta内の完了markerは
掃除しない。markerを読むコード/定数を消すことと、永続データ/schemaの破壊は別である。
他のsettings/タグ設定移行や一般XMP metadata/ratingの現役経路を削除対象へ広げない。

#### 後続実装の削除範囲・単一owner

chunk Cの実装担当が以下を一つの coherent removal として所有する。新しいtag writerは作らない。

| masterの削除対象 | 範囲・保持する境界 |
| --- | --- |
| src/indexer_manager.rs:129 / :225 | run_legacy_tantivy_tag_importとその唯一の呼出しを削除。専用perf/log報告も削除。現FtsMetaDb open → rebuild marker → wipe/FtsIndex openの順序は保ち、tags移行/proofを別位置へ移す処理は作らない |
| src/indexer_manager.rs:152 | progress文字列「旧タグをタグカタログへ移行しています…」を削除。§3.4/§4にもそのstage/日本語表示を新設しない |
| src/tags_db.rs:683 / :38 / :70 | TagsDb::import_legacy_tantivy_tags、LegacyImportReport、LEGACY_TANTIVY_IMPORTED_METAを削除。通常タグDB/write API、既存データは維持 |
| src/tags_db.rs:75 / :632 | source::TANTIVY_MIGRATION定数とTagsDb::metaは、削除後のcall-site監査で他用途が無ければ削除。現在の参照は移行本体と専用testsだけ。既存source文字列/marker行はそのまま保持 |
| src/fts_index.rs:668 / :682 / :240 / :832 | collect_legacy_tag_docs_at、private collect_legacy_tag_docs_from_index、LegacyTagDoc、stored_text_fieldを、他用途が無ければ削除。下記監査では移行専用なので撤去対象。通常reader/search/stored metadata読取は維持 |
| src/tags_db.rs:1452 / :1480、src/fts_index.rs:1004 | legacy_tantivy_import_copies_only_hash_tags_once、legacy_tantivy_import_skips_decided_items、collect_legacy_tag_docs_reads_stored_tags_onlyを削除。通常タグ/閉じたFTSタグ検索の回帰は削除しない |

**reader参照監査（2026-10-06）:** git grepとrepository検索で確認した。
collect_legacy_tag_docs_atの本番callerはindexer_manager.rs:155の移行だけ。
他のcallerはfts_index.rs:1023の専用testのみ。private helper、LegacyTagDoc、stored_text_fieldもこのreaderに専用。
移行と専用testを撤去すると他の利用は無いので、reader一式も撤去範囲に含める。
実装時はmerge後の全tracked source（tests/benches/vendor含む）で再確認し、新callerがあれば共用部分は残す。
sample_doc_with_tags（fts_index.rs:965）は別のFTSタグ閉鎖test（:1824）にも使うので維持する。
Tantivyのtags field/共通schemaをこのために削除・version bump・再構築しない。

#### 後続実装のdocsとrelease説明

| 文書 / 実装担当の更新範囲 | 更新内容 |
| --- | --- |
| docs/tag-catalog-redesign-plan.md D14/§7.1 | D14を2026-10-06決定で撤去した仕様へ更新。§7.1は導入時の歴史として残し、実装完了時に撤去済みと明示。旧版直接更新の非取込と既存catalog/XMPの保持を記載 |
| 同 D15/§2/§5.4/§8/§9/未解決事項 | 「STORED tagsは移行専用」「移行はwipeより前」「現役の移行挿入点/移行tests/性能計測」を現仕様から外す。tag_item_state/source/markerの歴史値を保持する説明、他の現役タグ設定migrationは維持。§7.2（v3.9.1）/§7.3（v3.4.0）の既廃止statusと撤去日/releaseの区別を保つ |
| docs/search-architecture.md §4.8/§4.9/§6のタグ背景 | 起動時Tantivy→tags.db取込・marker依存を撤去済み仕様へ更新。旧XMPを「移行元として読む」現役経路のように書かない。一般XMP/通常タグwrite/sidecar復元/検索分離は維持 |
| 本plan、async/ui-responsiveness等の現役参照 | 実装時の最終仕様へ更新。新しいadmission/notice/非終端migration eventを追加しないことを確認。歴史・archiveの記録は過去の記録と区別し、全面書き換えない |

**CHANGELOG候補（次release用。CHANGELOG自体は今回編集しない）:**
「v1.0〜v1.3の旧タグを検索索引からアプリ内タグへ自動移行する処理を終了しました。
v1.4.0以降で取り込み済みのタグはそのまま残ります。v1.0〜v1.3から直接更新する場合、
ファイルのXMPに残る旧タグはアプリ内タグへ取り込まれません。」
release leadが次releaseの説明へ採否・配置を判断する。起動停止の原因確定やGPU問題解消とは表現しない。

実装の回帰・参照消滅確認は§8のchunk Cに含める。今回この文書以外の上記docsは編集せず、
製品コード/CHANGELOGにも変更を加えない。

### 5.4 機能ごとの「準備中」契約

Indexer Pending は全サービスの readiness ではない。Unavailable も全文/アイテム索引についての終端で、
独立サービスの状態を上書きしない。各行の既存 request/read owner が入力・取消・再評価を所有する。

| 機能 / master 経路・owner | Pending の契約 / Ready・Unavailable の扱い / 試験 |
| --- | --- |
| Ctrl+G アイテム検索 / src/global_search_ui.rs:2210, :2247, :2280 / global_search owner | query/filter入力を保持し「検索の準備中」。Pendingを「インデクサが利用できません」へ落とさず、manager取得より前に結果準備workerやlast_executedを更新しない。Ready採用が同じ debounce owner を一度wakeし、**まだ検索viewがactiveで現入力が非空の時だけ**最新query/filterを既存経路で実行。閉じる/別navigation/空入力なら実行しない。別のpending query queueは作らない。Unavailableは終端の説明に切替、入力を保持し自動再試行しない。Pending中のA→B、閉じる、空、失敗、Readyとdebounce同時でBだけ一回を試験 |
| Ctrl+S コンテナ検索 / src/app.rs:27953, :28022 / favsearch + search_index_db（名前索引） | Indexerと独立に通常実行。名前索引自体の準備/失敗は既存契約を維持。Indexerを止めても名前検索結果が得られ、late Readyが結果/queryをresetしないことを試験 |
| Ctrl+F 現在地フィルタ / src/app.rs:63859, :63932, :64006 / search owner | 現items/on-demand metadata workerを維持。Indexer不在でも通常実行。独立 worker の取消/新query/採用を保つ。Pending/Unavailableの両方で結果・取消とlate adoption非干渉を試験 |
| タグ / src/tags_db.rs、src/tag_write_worker.rs / 既存読取・write owner | catalog/writerは独立。IndexerPending/Unavailableでも通常の編集/読取/sidecar復元/Undo/Redoを維持。旧移行を§5.3で撤去し、marker確認・準備中notice・queue・追加admissionは不要。タグservice自身の既存失敗は保持。タグ操作成功・永続化・lateReady非干渉を試験 |
| スマートフォルダ / src/app/smart_folder.rs:2124, :3578 / smart-folder request | 独立scan/DB読取を維持。managerなし時の既存local I/O semaphoreをそのrequestのまま使い、late Readyでworkerを作り直さない。タグ条件も現tags.dbを通常どおり読む。独立条件/タグ条件のscan成功・取消、semaphore owner数、late Ready非干渉を試験 |
| コレクション / src/ui_dialogs/collections.rs:43, :723, :1991 / collection actor + UI request | collection固有のStarting/Ready/Unavailableとread demandを維持。IndexerPendingで新しい待ちを足さない。collectionStarting中の要求保持、Indexer停止中のReady読取、late adoption非干渉を試験 |
| Remote / src/remote_ipc/collections.rs:167, :310, :341、src/remote_ipc/ui.rs:4002 / 各service + session owner | 名前検索・タグ・collectionは各serviceの準備状態。全文索引のPendingへ一括変換せず、タグ移行理由の新Busy/wire状態も追加しない。Remote取得は既存cancel、初期explicit延期、返却時fallbackとowned wakeを保持。running-instance activationも別ownerへ新startupを作らない。サービス毎の成功/固有待ちと取得→返却→取消を試験 |
| similar / 別バージョン索引 / src/app.rs:26123, :26893、src/indexer_supervisor.rs:377、src/similar_index.rs:1880 / 各registrationと既存readiness集約owner | overlay解除/normalreadyで共有watcher barrierを閉じない。実bootstrapのready、またはIndexer終端Unavailable/disconnectの既存terminal tailでのみ閉じる。Pendingで独立similarを早期失敗にせず最新password設定を保つ。UI先行→barrier継続→latebootstrap、terminal失敗一度閉鎖を試験。watchdog対象外でもこのproduct契約は維持 |

## 6. 初期フォルダの解決・走査を通常 UI 後へ

### 6.1 同じ要求に target / 解決 / scan / continuation を持たせる

起動入力を UI がメモリだけで snapshot する。mode、legacy fallback、カーソル、明示 argv、
§1.335 の versioned restore record を一つの typed input にする。
既存の `startup_open_path` と `startup_open_path_resolve_pending` を、同じ resolver owner の
`Deferred(input) / Resolving(pending) / Idle` にまとめる案とする。
Deferred は normal root present と既存 admission 待ち、Resolving は worker / result、Idle は要求なし。
新しい `pending_default_folder`、`pending_restore_scan`、`startup_opened` bool は加えない。
`initialized` は従来の AI 等の初回 setup に留め、フォルダ open 完了の代理にしない。
activation の held request、bookmark request の既存所有規約は型の payload へそのまま移し、再設計しない。

normal UI の最初の root present 後、同じ既存 resolver worker を dispatch。
worker が `known_folders::startup_folder`、Shell known-folder、存在検証・祖先遡上・format classification を実行する。
Directory ならその **同じ worker** で `scan_directory_with_convertible_archives_cancel`
(`src/app/folder_scan.rs:605`) と必要な既存 folder materialization を済ませ、`ScannedDir` を結果 payload に入れる。
UI には `Path::is_dir/exists`、Shell 照会、再 scan、cold catalog open を残さない。
scan は `DirEntry::file_type` の既存規約と cancel 確認を使い、失敗を空一覧に変換しない。

`src/app.rs:49202` の folder-pane open と `:48649` の DFS navigation は、completed scan → 通常 install の再利用例。
startup を「ツリー click」と偽って `PaneNavigation` に流さず、既存 owned loader / classified preflight と
`load_folder_with_scan_owned` (`:24695`) → `install_scanned_folder_listing` (`:25655`) の採用 seam を使う。
prepared scan と実フォルダ分類がある経路を追加・共有し、通常の同期 loader をもう一度呼ぶ経路に戻さない。
shared installer 内の metadata / persisted cache read は必要な prepared payload へ移し、UI への初期到達での同期 I/O を監査する。
フォルダの items install、sort / cursor / history / settings の採用契約は既存 navigation が担当する。

ZIP / PDF / EPUB / converted archive は既存の enumerate / password / conversion owner へ同じ continuation を渡す。
archive cache lookup / stamp / format probe が同期なら既存 background preflight を使う。
初期 requested image / video / audio が親 directory へ解決される場合は、completed scan 採用後の
`open_loaded_file_fullscreen` (`src/app/startup_ops.rs:1355`) に同じ required-file continuation を渡す。
default target は従来通り `auto_fullscreen=false`、明示 argv / activation の既存 auto-open 条件を維持する。

**Deferred の起床契約（P1-2）:** 要求を保持する既存 navigation owner が root wake を所有する。
overlay 解除時に normal paint 待ちを開始し、ROOT を明示した request_repaint_after_for（100msを提案）を
normal present 未成功の各 root pass で再武装する。Indexer の Pending/終端と切り離す。
normal-present callback は生きた要求 handle に一回の request_repaint_of(ROOT) を発行し、
次の外側 root poll が milestone と admission を検査して Deferred → Resolving を一度だけ遷移させる。
callback は navigation state を書かず、退役 handle への遅い callback は dispatch を再生しない。
timeout/skip/surface再作成の場合は milestone を立てず、同じ owner の timer を維持する。
これは未完了の所有処理を進める wake であり、描画症状を隠す任意の repaint 追加ではない。

利用者の hide/OS minimize 中は normal present 待ちの定期要求を止め、既存 restore/unminimize owner が
同じ Deferred 要求を wake する。modal/Remote admission に移った後も、解除/返却/取消を扱う既存 owner が
その要求を再評価する root wake を発行する。独立した resume worker や待ち bool は加えない。
すでに normal present 済みなら再描画成功を要求せず、admission が開いた pass で dispatch する。
Resolving の worker result/cancel/disconnect も同じ request owner に ROOT wake を渡す
（現 master の worker は startup_ops.rs:267 で current viewport の repaint を要求するため明示 ROOT へ揃える）。
ユーザー navigation の受理で Deferred が退役したら timer/登録も退役し、旧要求を開き直さない。
eframe が Wait に戻る境界（vendor/eframe/src/native/wgpu_integration.rs:958）に依存した無入力停止を
防ぐ契約であり、hidden render scheduler 自体は再設計しない。

### 6.2 target の意味、取消、兄弟経路

| 入力 | 保持する仕様 |
| --- | --- |
| Desktop | Desktop、開けなければ legacy last_folder の祖先。Shell/USERPROFILE fallback も `known_folders` のまま |
| Specific | 指定場所/祖先 → Desktop → legacy last_folder/祖先 |
| Previous（現 master） | legacy last_folder/祖先 → Desktop。空 path Drive sentinel と cursor 同じ場所条件 |
| Previous（§1.335 merge 後） | `startup_list_restore` の logical target と cursor。Unavailable / ancestor / Desktop fallback は同枝の契約 |
| Drives / ReadingHistory | mode routing は維持。ドライブ照会/必要な DB 読取があれば同じ prepared worker に載せる。通常 UI を先に描く |
| 明示 argv、SendTo、activation | default より優先。requested file / auto fullscreen / password / converted-source 意味を維持 |
| 明示 open の NotOpenable | 現 `startup_ops.rs:508`, `:523` の default fallback も同じ非同期 default input へ。Explicit→Default 一回だけ。Default 自身の失敗は terminal にし、default worker を再帰起動しない |
| Remote fallback | `src/remote_ipc/ui.rs:4002` の default 入口も同期処理を呼ばない。Remote ownership の既存 defer/reject を保持 |

Deferred はユーザー navigation が受理されたら退役。Resolving は同じ既存
`cancel_unresolved_open_for_navigation` (`src/app/startup_ops.rs:696` 前後) から cancel / receiver drop。
結果が来ても owner / request identity / main navigation 世代が違えば採用しない。
新しい navigation 中に遅い Desktop/Previous が着地して上書きすることは許さない。
navigation supersession 時の record/cursor の rollback journal は作らず、成功採用まで保存しない。
初期要求の error / unavailable / cancel は通知・timeline terminal を出して通常 UI に留まり、overlay を再表示しない。
activation / bookmark に既存の modal-held behavior がある場合はそれを保ち、modal を横切って強制採用しない。
Remote が初期 explicit target を延期する既存 semantics も保持。target ごとに new resume owner を増やさない。

admission 待ち以外のパス解決/scan全体はモーダルにしない。モーダルを無期限に保つと本目標と矛盾し、
遅い share が別フォルダへ移る操作まで止める。代わりに既存 navigation の取消・単一採用を使う。
変換・パスワード・保存は既存モーダルのままで、今回その内部の interleaving を増やさない。

### 6.2.1 resolver の spawn failure / disconnect（P2-3）

startup_ops.rs:297 の同期 fallback を InitialStartup / Activation / Bookmark の **全経路**で削除する。
同じ resolver に scan を載せた後も UI thread で resolve/scan を行わない。spawn error と実行中の channel
disconnect は、既存 owner と requested resource lease を受ける共通 terminal tail に渡す。
記録と一度の「場所を開く準備ができませんでした。もう一度開いてください」通知で終わる。
自動 retry や代替 worker は作らず、次に利用者が open すれば通常の新要求になる。

| owner | 失敗時の終端処理 |
| --- | --- |
| InitialStartup（default含む） | 当該 Deferred/Resolving を Idle にして requested EPUB 等の lease を解放し、trace を terminal にする。通常 shell/current list を保持し overlay を再表示しない。リソース失敗を NotOpenable と混同して default resolver を再 spawn しない |
| Activation | 新 activation を terminal にし、当該 lease を解放。snapshot admission のため保留した prior request があれば resume_activation_held_open（startup_ops.rs:543）の既存契約でその **同じ receiver/要求**を戻し、実 held duration を bookmark timeout へ加算する。新 activation の失敗で held request を取り消さない。通常 supersede により受理時に取消済みの旧要求（:208/:240）は復活させない |
| Bookmark(request_id) | 同じ ID の cancel_bookmark_open_request（:769）へ渡し、当該 bookmark の pending/lease/preparing host を既存 terminal 所有規約で退役する。他の bookmark、parked viewer、sibling の要求を取消しない。共通通知で一度だけ説明する |

spawn が失敗して worker が存在しない場合も cancel handle を terminal にし、held 要求の elapsed は
受理から失敗判明までで計る。結果 channel が途切れた場合も成功扱い・空 scan 採用をしない。
従来の Initial disconnect → default（:381）は自動の新 worker を作るため、このまれなリソース失敗では
上の粗い終端処理へ統一する提案。通常の「指定場所が開けない」時の一回の default fallback は維持する。
rare failure の多段 recovery は設計しない（§9 の設計 lead 確認事項）。
RequestedOwner/held-request の退役を分岐ごとに複製せず、既存 finish/cancel/resume tail を共有して通知の
二重発行も防ぐ。Detached preparing host の具体的な terminal helper 変更は §7 の構造合意対象とする。
同じterminal tailでも終了/取消済みownerへの遅い失敗ではnotice/held復帰を行わず、lease解放だけを行う。
App終了時はheldも当該要求として退役させ、以前の場所を再openしない。

### 6.3 §1.335 と compose、merge 順序

読み合わせた別枝: `C:\home\mimageviewer-v430still` / `next-startup-restore`、
`docs/startup-restore-target-plan.md` §5.1 / §5.2 / §5.3 / §10.3。
同計画の Phase 1 は `23d885e11`、初回実装 `83d2ac3c1`、§10.5 の追加レビュー対応中の文書を参照した。
別枝コードの `src/app/startup_list_restore.rs:87` は Previous input から
`StartupListIntent::RestoreList { target, cursor }` を既存 owned navigation に渡す。

**推奨 merge 順序: §1.335 を検収・master 統合 → 本枝を rebase → plan A 実装/統合。**
first-setup overflow 枝は first_setup.rs のみを独立統合でき、本枝は触らない。
同時 writer を置かず、本枝の `startup_ops.rs` / `app.rs` 実装は §1.335 の対象変更を取り込んだ後に行う。

境界の契約:

- §1.335 は **何を復元するか / いつ明示一覧を記録するか**の正本。本枝は **いつ/どこで解決・scan するか**の owner。
- Previous の snapshot は versioned target + cursor をそのまま worker へ。legacy `last_folder` に戻して新仕様を消さない。
  Desktop / Specific が使う legacy fallback は残し、Previous の毎回 legacy migration を足さない。
- `StartupListIntent::RestoreList` と logical source / zip_prefix を結果、enumerate、変換 continuation まで運ぶ。
  prefix を filesystem path に join しない。§1.335 の最終 prefix 検証・階層再構成・カーソル適用・一度の採用を再利用。
- worker の解決完了 / intermediate ZIP root / first present では restore record を書かない。
  最終 main list adoption だけが §1.335 の reducer へ通知。ancestor/prefix fallback の cursor 破棄条件を保つ。
- `startup_list_restore` の schema、migration、main-list intent、F12 close の record 規則は本枝で変更しない。

逆順の統合しかできない場合は、§1.335 の Previous helper を同期実行へ戻す semantic conflict を
設計 lead と解消してから再検証する。git のテキスト merge 成功だけで compose 完了にしない。

## 7. 簡素化と detached への到達

検討した簡素化:

- 全 init をモーダルで待つ: 現状そのものが無期限 overlay を生むため不採用。
- deadline で検索を失敗扱い/worker cancel、次回再実行: 普通の遅い起動で既存検索を失うので不採用。
- second writer / retry / restart / 設定差分 replay queue: 不採用。同じ Pending と既存最新 configuration submit で足りる。
- 検索が準備できるまでお気に入り設定を禁じる: 普通の設定操作を減らすため不採用。既存最新設定同期を使う。
- folder resolver と別の startup scan owner: 不採用。同じ request が解決/scan/continuation を運ぶ。
- 起動 target の高速化のため Desktop を空一覧へ変更: semantics 退行なので不採用。通常 shell を先に出し、同じ target を開く。
- 画面を後から専用に live-rebuild: 不要。通常 navigation の既存 install / adopt を使う。
- 旧タグ移行と編集をqueueで順序付ける/overlayを延長する/タグ操作を一時制限する:
  §5.3の2026-10-06利用者決定で全案不採用。まれな旧版直更新の自動救済を撤去し、
  競合の組合せ自体をなくす。通常タグwriter/admissionとsidecar ownerは変更しない。
- Remoteの論理初期要求へwatch slotを保持し、返却/繰返し取得/fallbackで再開する診断契約:
  再レビューの指摘に対し設計ownerが不採用を決定。初回dispatchだけ監視し、Remote取得で
  suspended (not watched) terminalにする。業務上の延期/再dispatchは既存ownerのまま、timelineだけ残す。
- metadata registration generation/readinessを新しいwatchdog集約ownerへ結ぶ:
  不採用。既存per-supervisor registrationとsimilarの集約にはbegin/end eventだけを置き、
  watchdogの監視/終了条件から外す。診断のために正しさのlifecycle機械を増やさない。

Indexerとnavigationの業務状態は既存の分割状態を enum に置換する範囲。
タグの移行安全性state、proof event、追加notice/queue/保留処理は導入しない。
タイマー・診断 stage は観測の owner であり、
App の機能状態を書き換える第2 coordinator にはしない。まれな診断 I/O / spawn failure は
記録可能な範囲のログと一度の通知 + 次回起動に留め、回復の失敗をさらに回復する仕組みを作らない。
利用者作成 settings / 現tags.db / collections の削除・defaults 上書きは承認されていない。
例外として、未移行の旧タグを新たに救済しない仕様変更だけを§5.3で明示的に受け入れている。

**detached / viewport 構造合意が必要な箇所:**

1. first-present hook は `vendor/eframe/src/native/wgpu_integration.rs:877` と共通
   `vendor/egui-wgpu/src/winit.rs:479` に触れる。root-only event を足すが、同 painter の deferred/immediate viewport が通る。
   paint skip / texture delta の既存契約を変えず、root milestone を detached paint で確定しない構造変更。
   §6.1の要求handleに対するroot wake/最小化通知もこのobserver境界の一部として合意対象に含める。
2. Directory prepared-scan を使う loader は `src/app.rs:24695` / `:25655` の shared path。
   context-owned folder pending と history/navigation owner、§1.335 intent を保持する必要がある。
3. 明示 startup file の既存 fullscreen continuation は detached host を作り得る。
   `src/app/startup_ops.rs:874` 以降には bookmark の detached predicate 群もある。
   初期/default の main owner を明示し、共有 helper の変更が sibling branch に及ばないか review する。
   §6.2.1のBookmark失敗時にpreparing hostを退役する既存terminal helperへの到達も同じ合意対象。
4. 外側 root update への App-global Indexer poll 移動は context mount/swap で manager を移譲しない。
   新しい detached predicate、HWND owner 推測、placement、viewport generation、terminal close effect は追加しない。

`docs/detached-rework-plan.md` §2 は読了。上記は起動の証拠/ownership の構造変更案で、
症状を隠す delay / geometry guard / extra repaint ではない。ただし **合意済みとは扱わない**。
実装前に設計 lead（ClaudeCode）と独立 Codex reviewer がその主張に合意し、
具体的に触る範囲と理由を同計画 §11 に記録する。本 Phase 1 では同計画自体を変更しない。

## 8. 検証、実装 chunk、担当

実装/自動検証の担当は Codex GPT-6.1 Sol / high。一つの coherent chunk の共有 file は一人が編集。
独立設計/実装レビューは別コンテキストの Sol / xhigh、設計・検収・公開は ClaudeCode Opus / high。
この文書は実装担当による Phase 1 提案で、設計 lead の決定や独立承認を代替しない。
本 Phase 1 では product code、実アプリ起動、ビルド、コミットをしない。

| chunk / owner | 実装範囲 | 必須の自動試験 |
| --- | --- | --- |
| A: timeline process owner / writer | 共通小 crate、launcher/core entry、全stage記録、限定watchdog（§3.5） | fake monotonic clockでbegin/end・error・skipped・parent duration、対象経路の5/15/30秒各一回、heartbeatが動いても発火、stage切替/reset、並行timeline、overflow/drop、writer blocked/failedでもpublisher完了、UNC/extended UNC/mapped/unknown、env上限/不正/直接core/同run相関。対象外の再dispatch/metadataはtimelineだけでwatchdogを延命/再起動しない。遅いsinkはlatchで止め、UI側が待たないことを実APIで確認 |
| B: root presenter の証拠 owner | vendor の observer / frame tag 接続、outer first-update | fake paint outcome: surface absent/timeout/recreated/zero-size/skip/submit-only は成功なし、present-return だけ一度、root以外なし、Overlay→NormalShell、multi-pass/discard の最終tag、first-updateだけでreadyにならない。既存 texture-delivery / font-atlas / surface regression を保持 |
| C: App-global IndexerInit owner | enum、全consumer、単一採用、overlay/status、§5.3の旧タグ移行撤去（code/test/docs） | 実 receiver と injected spawn で4.999秒/5秒/期限同時Ready、恒久Emptyでも通常UIへ、30秒Ready後一度採用、Unavailable/Failed/disconnect/spawn失敗一度通知、poll early-return/idle、Pending中のfull-check集約、favorite ON/OFF/root/PDF password変更後に最新configuration採用、speed変更は次回起動反映のまま、similar bootstrap非早期閉鎖、close前late-result、manager/writer生成数1、旧移行非実行と既存タグ保持 |
| D: 既存 initial navigation request owner | target snapshot、defer until present、resolve+pre-scan、async adopt、§1.335 compose | fake resolver/scanを停止して先にNormal root present・navigation可を確認。再navigation/activation/Remote取得/closeでstale結果非採用・旧trace非再活性化、modal-held保持、scan errorと空を区別、default fallbackは一度。実temp directory/ZIP/PDF/変換fixtureでtarget・cursor・source・prefix・auto-open保持 |
| E: pure UI renderer | Japanese overlay / persistent status | fixed clock snapshot: light/dark/strong contrast、480×360、小viewport、長いstage名/999秒、Overlay/Pending/Unavailable。glyph check zero。first_setup snapshotを更新せず別枝責任 |

独立レビューに対応した追加必須試験（上表の担当を再利用し、第2検証ownerを置かない）:

| 担当 / 対象 | 操作と不変条件 |
| --- | --- |
| A / parent watchdog | fake clockで親40秒の下に1秒のchildを連続。parentの5/15/30秒が各一回、parent復帰でも再通知なし。stage用thresholdとparent用thresholdを混同しない。heartbeatは更新し続ける |
| A / 監視寿命 | normalready後もIndexerを40秒止めて記録。run_nativeが100秒常駐するだけでは警告なし。modal admissionを100秒保留しても実resolver停止扱いなし。visible4秒→hide100秒→restore1秒でpresent-awaitの有効5秒が一度、Indexerの実停止はhide中も記録。自身の全slot terminal/quitでwatch終了、timelineは継続 |
| A+D / 初回dispatch・Remote除外 | 初回resolverを停止→5/15秒の対象警告→Remote取得でsuspended (not watched) terminalを記録。初回slotがRetiredとなり他の対象slotも終端ならwatchdog終了。返却→同じ初期argvの再dispatchを40秒停止しても新overdue/slot再利用/watchdog再起動無し、timeline beginは残り完了時にend。繰返し取得/返却、初回dispatch前取得、初回失敗→default fallbackも同じ除外を確認。Indexer Pendingが残る場合はその監視だけ継続 |
| A / metadata登録除外 | 複数supervisor登録の一つを停止。reconfiguration workerのspawn/idle記録がその登録のendにならず、当該registration beginだけが残ることを確認。normalready・Indexer採用・初回dispatch終端でwatchdogが終了し、40秒以上の登録待ちでもoverdue/延命/再起動無し。登録が返れば同じregistrationのReady/Unavailable endをtimelineへ記録。既存similar集約/barrierは別ownerのまま維持 |
| B+D / Deferred wake | Indexer ReadyまたはUnavailable、他worker全idle、無入力のfake event loopをWaitまで進め、normalpresent完了のowned wakeのみでdispatchが一回起こる。skip/timeoutを複数回→成功でもtimerが継続。cancel前後の遅いcallbackは再openしない。複数egui passで未採用frame tagを使わない |
| C / visibility採用 | Pending→tray hide→Ready（hiddenのまま実outer pollで採用）→restore。同じmanagerのthrottleがtrue→false、設定変更/ActivityGateを巻き戻さない。pause設定ON/OFF、外部ShowWindowによる復帰、hidden quit Pending/Ready、late result非採用を分ける |
| C / minimize/activation区別 | OS minimizeだけではwindow_visibleや新throttle契約を変更しない。runninginstance activation→restoreでは新init/overlay/defaultfolder無し。起動途中hideの結果採用はROOT requested-workで進み、hiddenidleへ恒久timerを作らない |
| C / legacy移行撤去 | disposable旧索引（STORED #タグあり）と空tags.dbで既存store startupを実行し、旧タグ非取込・marker非新設を確認。既存catalogのタグ/source/state/markerは有無にかかわらず起動/rebuild後も保持。XMP fixtureのbytes/hash不変。Indexer停止→5秒UI解除中もAdd/Remove/改名/Undo/Redo/sidecar復元、タグ保存copy/move/delete/metadata操作とRemoteタグ読取に新制限無し。専用tests削除後も閉じたFTSタグ検索の回帰を維持。実データは使わない |
| C / サービス別契約 | §5.4各行をPendingとUnavailableで検証。Ctrl+G最新query一回/取消、Ctrl+S/F通常動作、tag/smart/collection/Remote個別readiness、共有watchbarrierをoverlaydeadlineで閉じない。lateReadyは独立要求をresetしない |
| D / resolver resource failure | 既存FORCE_STARTUP_OPEN_RESOLVE_SPAWN_FAILUREをInitial(default/explicit)/Activation/Bookmarkへ注入。resolve/scanの呼出し数0、UIからsyncfallback無し、通知一回、lease/trace退役。Activationのheldあり/なし・snapshot拒否・交代activation、Bookmarkのmain/detachedを分け、prior receiver同一・timeout補正・sibling非取消を確認。disconnectも同じterminal tail。NotOpenableだけ一回defaultfallback |
| D / admission wake | normalpresent後modal終了/Remote返却で同じ要求をwakeし一度dispatch。意図的hide/minimize中はdispatch/presentを捏造せず、restore後ownedwakeで再開。Resolver結果のwakeもROOTへ届くことを確認 |

folder 回帰 matrix は Desktop / Previous / Specific / Drives / ReadingHistory、missing child→ancestor、
missing drive→Desktop、network/OneDrive解決停止、カーソル同一場所だけ、argv file / ZIP / PDF / EPUB /
converted archive、password取消、Ignore/Refused、Remote fallback を含める。
§1.335 統合後は parent list→book→quit→Previous、明示book listへの復帰、ZIP内部prefixとBackspace、
Drive sentinel、new record migration、直接読書、A/B、tray resume の同枝回帰を再利用・追加する。
multi-context は main のstartup scan待ち中に independent/parked viewer を保持し、
items/generation/channel/cancel/cache/restore recordが siblingに及ばないことを実handlerで確認する。
headless試験のnormal-presentは実painter証拠の代替ではなく、同一adoption APIに fake outcomeを注入する。

実装順は A → B → C → D → E、各chunkの終了で関連試験と独立review。
§5.3の撤去はchunk C内でコード・専用tests・現役docsを同じ担当が完結させる。
全source参照監査で専用関数/型/文字列が消えたこと、共有helperを過剰削除しないことを確認する。
CHANGELOG候補は本planからrelease leadへ引き渡し、今回の設計作業ではCHANGELOGを編集しない。
最初は narrow lib / 指定integration test、共有startup/viewport完了時には
`scripts/test-full.ps1`、`cargo fmt --check`、`python scripts/check_ui_glyphs.py`。
docs/README、async-architecture、ui-responsiveness、仕様/ユーザー説明は実装で変わった最終仕様へ更新する。

実機は事前に具体的suite・所要時間・desktop使用・disposable data scopeを提示し、明示承認後にのみ実行。
候補suite: portable-smoke のcold/warm起動、5秒以上/30秒以上のIndexer停止注入とlate completion、
遅い初期directory/新navigation、root present証拠、ZIP Previous prefix（10–15分、desktop/input使用）。
`prepare-portable-smoke.ps1` の disposable copyだけで、normal profileをagentが起動しない。
AV有効clean VM / Store同silent installer / 実GPUのsigned artifact検証は公開leadの別session。
実装後はlauncher/vendored renderingに到達するためrelease verification buildを用意し、
利用者へ `Start-Process -FilePath .\target\release\mimageviewer.exe` を引き渡す。
通常 `%APPDATA%\mimageviewer` の実settings/dataを更新し得ること、installed/tray常駐を先に終了することを明記。
本Phase 1は文書のみなのでverification binaryを作らない。

## 9. follow-up と設計 lead への確認事項

**follow-up: launcher-supervised backend fallback。** core entry / first root present / normal-ui readyを
launcherが期限付きで監視し、native callが返らない時はGPU非依存の失敗説明を出す。
fallbackを試すならDX12列挙を含まないVulkan-onlyまたは明示software経路を子process単位で有限回試す。
native FFIにfuture timeoutを足すだけでは停止threadを解放できない。子processの停止/既存mutex/
設定migration/optional bridge ownershipを含む独立設計が必要で、plan Aに混ぜない。

**利用者決定済み（2026-10-06）:** §5.3の旧Tantivyタグ一括移行撤去と、旧版直接更新での
旧タグ非取込を受け入れる。タグ制限/queue/準備notice/proof機構と、それらに付随する移行失敗の
復旧案は不要となった。通常タグ操作と既存catalog/XMPを保持する。この決定を改めて承認待ちにしない。
現時点で追加の必須利用者判断はない。実装範囲、docs更新、CHANGELOG候補は§5.3へ記録済み。
**watchdog範囲も設計owner決定済み:** §3.5のcore startup経路と初回dispatchだけ。
Remote取得で当該監視はsuspended (not watched) terminal、再dispatch/metadata登録はtimelineのみ。
この対象外処理のためにwatchdogの寿命を延ばす/再開する設計は行わない。

設計leadの確認事項: resolverのspawn/disconnectは通常UIに留め、一度通知して利用者の新openを待つ
（§6.2.1）。従来disconnect時の自動defaultへの新workerは作らない。diagnostics保存不能、
Indexer spawn/errorも一度通知・次回起動に留める。rare failure内の多段 recovery は増やさない。
レビューで確定する提案値はoverlay5秒、watchdog5/15/30秒、診断1024件/24KiB handoff、2MiB/file・10run保持。
再レビューでは旧移行撤去/P1-2/P2-3/4/機能表等は受入れ。残るwatchdog P2×2は本改訂で
監視範囲の限定により扱う。今回の限定変更の再確認と、実装前のdetached §11記録はまだ必要。
より複雑な復旧が必要なら実装前に相談する。画面前の同期停止まで正常UI保証が必要という判断なら、
今回のscopeを拡張せず、上記launcher監視か必須初期化の背景化を別のcoherent phaseとして相談する。

## 10. 実装分割（2026-10-06利用者指示）

f2fcef6b2は独立設計レビューready、指摘なし。detached到達部も構造変更として設計owner
ClaudeCodeと独立Codexが合意済み。Phase Aの正確な到達scopeはdetached-rework-plan §11へ記録した。

Phase Aは常時launcher/core timeline、実root PRESENT、stage/elapsed overlay、Indexerの
5秒上限・同じworker継続・単一late adoption・ROOT wake・現tray throttle・Unavailable、
狭いwatchdog、旧Tantivyタグ移行撤去を実装する。§6の初期フォルダoff-UI化、resolverの
同期fallback撤去、Deferred normal-present wake ownerはPhase Bへ保留する。
first_setup.rsとCHANGELOG.mdは変更しない。変更候補は§5.3のままrelease leadへ渡す。

Phase Aのfirst-dispatch watch終端は既存resolver結果のUI admission／同期default loaderのreturn。
非同期page enumerationやconversionの完了を意味するmilestoneではない。それらの既存ownerを
今回移動・再設計しない。Remote取得はreserved/active initial watchを
`suspended (not watched)`として退役し、返却再dispatchはtimelineのみ。metadata registrationも
supervisorごとのtimelineだけでwatchdog寿命を延ばさない。NormalPresentは実際のnormal grid描画要求で開始し、
既存tray hideとOS minimize中は同じ時計を停止する。CoreStartupは最初の実root PRESENTのreturnまで監視する。
既存の起動fullscreenがnormal gridを通らない場合はNormalPresentをSkippedで退役し、readyを捏造しない。
未採用Readyが入ったreceiverの破棄も終了時に背景disposalへ移す。破棄workerさえspawn不可な
まれな終了時失敗は記録し、その派生index所有物をprocess終了まで保持する。UI同期shutdownやretryはしない。

先に§1.335 next-startup-restoreを検収・統合し、本枝を更新してからPhase Bを実装する。
Phase Aの自動gateはnormal／portable／portable+test-script check、fmt、diff、glyph、
full lib testと共有診断・vendor・launcherのfocused tests。利用者指示によりbuild-devを用意し、
agentは起動しない。launcher extraction/release性能の実機確認はrelease leadの別検証枠に残す。

### 10.1 Phase A実装・検証記録（2026-10-06）

Phase Aを未commit差分として実装した。独立Sol/xhigh実装レビューは修正後ready、指摘なし。
最後の追加確認では、spinner snapshotを既存の固定frame方式へ統一し、PendingのCtrl+Gでも
メモリ内の無効favorite filter正規化を先に行うことを確認した。query保持・prepare/search非開始は維持する。

`scripts/test-full.ps1 -SuppressCrashDialogs` はPASS。本体libは10,659成功・52既存ignore・失敗0、
UI snapshotは94成功（起動表示6件のPNGを目視確認）、launcherは39成功、共通診断crateは21成功。
workspace除外vendorもegui 25／egui-wgpu 11／eframe 18成功。normal／portable／portable+test-scriptの
core check、workspace fmtと変更vendorのrustfmt、diff check、glyph lintも成功した。
本体の既存設定転送テストは、変更前policyに既にある`video_watched_to_end`を含む441項目へ
期待件数だけを更新した。転送policy・許可131項目・wire129項目は変更していない。
Susie統合試験には不足していた実体fixtureを補い、skipではなく8件成功を確認した。

利用者指定の通常feature `build-dev` は成功し、core／Remote／EPUBとVC runtime/PE検査を完了。
検証用アプリは起動していない。証拠と環境条件は`target/planA-msg-A.txt`、最終全体gateは
`target/planA-full-final-{stdout,stderr}.txt`、buildは`target/planA-build-dev-final-{stdout,stderr}.txt`。
Phase Bの初期フォルダoff-UI化・resolver fallback撤去・Deferred wakeは未実装で、§1.335統合後に続ける。

### 10.2 Phase A再レビュー対応（2026-10-06、bb32bc6e0後）

`target/rplanAimpl-review.txt`のP2×3・P3×1に対応した。通知はdiagnostics ownerに保持し、
overlay解除後かつroot可視・OS非最小化の時だけ消費する。遅いIndexerの同じreceiverを維持したまま、
overlay→5秒解除→tray非表示→最小化→復帰→新しいtoast期限→一回表示をhandler-level testで確認する。
child watchの128件目・129件目、超過ログ一回、既知child/別slotの監視停止、親期限と終端維持を
fake clockで検証する。保存先はancestor probeを注入して`C:\link\data`のUNC向けjunctionと
照会失敗を検証し、実ネットワーク・実link・通常profileは使わない。
`docs/spec.md`も旧移行廃止・直接upgrade時の非取込・既存catalog/XMP保持へ更新した。
独立Sol/xhighの再レビューはready、追加指摘なし。新しいdetached predicate/viewport経路は変更していない。
Phase Bの保留とmerge順は§10のまま。最終command・件数・exit code・build証拠は
`target/planA-msg-A2.txt`へ記録する。
