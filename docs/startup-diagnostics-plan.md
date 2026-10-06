# 起動診断と起動オーバーレイの有界化 — plan A

2026-10-06 / Phase 1 **設計のみ・未実装・独立設計レビュー前**。
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

- lane は固定の少数（launcher main、core main/render、Indexer init、initial navigation、metadata bootstrap）。
  実行 owner が begin/end を発行し、その lane の現在 stage と stage 開始単調時刻を atomic に公開する。
  子 span へ入る前後は owner-local な固定深さ stack で親を復帰し、親 total と子 duration を二重加算しない。
- current stage の ID と開始時刻は一つの `AtomicU64` に pack し、watchdog/UI が異なる世代の ID と時刻を組み合わせない。
  例: 16bit ID + run-relative 48bit microseconds。長い寿命を前提にせず startup 用とし、同じ stage 再入にも開始時刻で identity を付ける。
  diagnostics snapshot のための spin loop、seqlock 再読ループ、`try_lock` + sleep を作らない。
- event は固定サイズの構造体。固定 journal / queue への登録は一度の `try_lock`、取れなければ atomic dropped count を増やす。
  mutex 下で QPC 以外の OS API、format、JSON、ファイル I/O をしない。current stage は登録失敗でも先に publish する。
  journal は 1024 件 / process を上限とする。過剰時は diagnostics を欠落させ、起動を待たせない。
- 初期navigationのtraceはrequest ID付きのhandleをそのrequestが所有する。取消時にtraceをterminalにして
  live-stageの監視対象から外し、止まらない旧workerのlate eventで別requestのstageを上書きさせない。
  初期explicitと一度のdefault fallback以外の通常navigationへstartup laneを使い回さない。
  Indexerはprocess-wide、フォルダ要求はmain-context固有とし、ログの相関もこの境界を保つ。
- writer は queue から固定 batch を取り、guard を解放して JSONL encode / directory 作成 / write / flush を行う。
  begin、end、watchdog event は短い buffer のまま放置せず writer 側で flush。`sync_all` は要求しない。
  disk が止まっても publisher/watchdog/UI は進める。watchdog と writer は別 thread とする。
- launcher の handoff snapshot は journal の一度の非待機取得で作る。競合なら欠落数を付ける。
  launcher exit に writer drain の待機を追加しない。末尾の未出力はあり得るが、core に渡った pre-spawn snapshot で補う。
  core も終了時に writer の無期限 join / flush をしない。diagnostics の shutdown は best effort。
- writer / watchdog の spawn error、disk-full / permission error は diagnostic-unavailable をメモリへ公開して終わる。
  通常 logger へ再入して報告しない。起動後に通常 UI が通知可能なら一度だけ「起動診断を保存できませんでした」と表示する。
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
data_dir の canonicalize / ネットワーク接続の確認を診断目的で起動 thread に追加しない。
GPU 情報は既存 adapter 情報から backend / name / vendor / device / driver 情報を記録し、
診断のために別 GPU device を作らない。driver 欄が API から得られない場合は unknown。
root HWND visible / minimized / position / DPI は既存確定 root handle から一度記録する。
HWND の geometry 推測や全 detached window 列挙を常時 timeline に持ち込まない。

### 3.4 必須 stage の配置（begin/end 両方）

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
| legacy tags DB / collection / import | `src/indexer_manager.rs:129`, `:135`, `:155`, `:164` | 同じ worker。利用者 tags を timeout で消さない |
| rebuild marker read / old index wipe | `src/indexer_manager.rs:226`, `:244` | 同じ worker |
| Tantivy open / schema recreation / reader | `src/indexer_manager.rs:260` → `src/fts_index.rs:350` | 同じ worker。meta lock と writer lock を混同しない |
| inventory reset / marker complete | `src/indexer_manager.rs:274`, `:287` | 同じ worker |
| IndexWriter / dispatcher start | `src/indexer_manager.rs:422`, `:438` → `src/fts_index.rs:406` | 同じ worker、生成後の唯一の writer は既存 dispatcher |
| metadata runtime spawn / manager return | `src/indexer_manager.rs:446`, `:467` 前後 → `src/metadata_reconfiguration.rs:93` | startup-init worker |
| cleanup / reconciliation / supervisor spawn / watch bootstrap | `src/metadata_reconfiguration.rs:271`, `:345`, `:349`, `:518`, `:533` | 既存 metadata worker。別 lane で begin/end、UI-ready の待ち条件にしない |

timeline の観測を入れるために DB の recovery / 索引再構築仕様は変更しない。
背景 AI / WASAPI / Susie / VST / EffeTune / tray の開始と結果は補足として区別し、
それらが通常 UI の待ち条件であるような単一「起動中 stage」を上書きしない。

### 3.5 UI heartbeat と独立した watchdog

launcher entry / core entry に専用 watchdog を開始。250 ms 程度の timed wait で atomic lane snapshot を読み、
**同じ stage 開始時刻から 5 / 15 / 30 秒**を超えた各 threshold を一度だけ記録する。
UI heartbeat の増減で期限を reset しない。writer の disk write や普通の logger も呼ばず非待機 enqueue のみ。
親の await-indexer span も lane に保持するため、短い子段階を繰り返しても総待ち時間を見失わない。
並行する lane はそれぞれ記録し、例えば `normal_ui.ready` 後でも Indexer init の滞留は記録する。
5秒の warning と §5 の overlay limit は別の意味で、watchdog が App 状態を変更してはならない。

root が描けない driver 停止でも、最後の `adapter.enumerate.begin` / `device.request.begin` /
`d3d11.create.begin` / `present.begin` と overdue が残れば停止境界を特定できる。
UI heartbeat は添付情報であり、spinner 継続も証拠にできる。30 秒以降に同じ warning を連打しない。
startup target と Indexer init が terminal になるか process 終了まで監視。通常閲覧の毎フレーム記録には拡大しない。

### 3.6 first PRESENT と通常 UI ready の正本

`src/app.rs:84870` の既存 `first_frame` は first-update として保つが、present と呼ばない。
新しい証拠は **root の surface acquire 成功 → queue submit → `output_frame.present()` が戻った直後**
（`vendor/egui-wgpu/src/winit.rs:693`）。API は画面の実 pixel 可視や DWM 完了を保証しないので、
ログ契約は `first_present.returned` とする。API 内の hang は begin だけになる。
surface absent / timeout / lost / zero-size / paint skip / submit のみでは first-present を確定しない。

vendored `WgpuConfiguration` に任意の軽量 diagnostics callback を渡し、初期化 span と
root paint outcome を固定 event として core の timeline へ発行する。callback は publisher だけを呼ぶ。
App は root の該当 frame の draw が Overlay / NormalShell のどちらかを小さい frame tag に公開する。
eframe が root update の full output を paint へ渡す時にその tag を捕捉し、同じ paint attempt に対応させる。
egui の複数 pass / discard の場合は採用された最終 pass の tag を使い、前 pass の Normal tag を流用しない。
root と frame identity を一緒に渡し、deferred / immediate detached paint から root-ready を立てない。

最初に Overlay を present すれば `first_present.returned` のみ。
NormalShell tag の root present が戻った時に `normal_ui.ready` を一度 publish する。
App は次の root update でその atomic milestone を読む。**この後にだけ初期 target request を dispatch**する。
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
| legacy tags | 保存済みのタグを確認しています |
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
worker result、spawn failure、channel disconnect は同じ `adopt_indexer_init_outcome` に入る。
この関数だけが Pending → Ready / Unavailable を遷移させ、manager を install する。
初期化中だけ 100–200ms の `request_repaint_after` と残り overlay deadline の短い方を毎 pass 要求する。
通知のための第2 poll、writer 起動、timeout thread からの App 書換えは作らない。

採用時の順序:

1. Pending を `take` 相当で退役し、同じ outcome を二度採用できなくする。待ち時間と result を timeline に publish。
2. Ready manager を enum に移し、**採用時点の最新** favorites / excluded roots / PDF passwords /
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

### 6.2 target の意味、取消、兄弟経路

| 入力 | 保持する仕様 |
| --- | --- |
| Desktop | Desktop、開けなければ legacy last_folder の祖先。Shell/USERPROFILE fallback も `known_folders` のまま |
| Specific | 指定場所/祖先 → Desktop → legacy last_folder/祖先 |
| Previous（現 master） | legacy last_folder/祖先 → Desktop。空 path Drive sentinel と cursor 同じ場所条件 |
| Previous（§1.335 merge 後） | `startup_list_restore` の logical target と cursor。Unavailable / ancestor / Desktop fallback は同枝の契約 |
| Drives / ReadingHistory | mode routing は維持。ドライブ照会/必要な DB 読取があれば同じ prepared worker に載せる。通常 UI を先に描く |
| 明示 argv、SendTo、activation | default より優先。requested file / auto fullscreen / password / converted-source 意味を維持 |
| 明示 open の NotOpenable / disconnect | 現 `startup_ops.rs:381`, `:508`, `:523` の default fallback も同じ非同期 default input へ。Explicit→Default 一回だけ。Default 自身の失敗は terminal にし、default worker を再帰起動しない |
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

新しい業務状態は既存の分割状態を enum に置換する範囲。タイマー・診断 stage は観測の owner であり、
App の機能状態を書き換える第2 coordinator にはしない。まれな診断 I/O / spawn failure は
記録可能な範囲のログと一度の通知 + 次回起動に留め、回復の失敗をさらに回復する仕組みを作らない。
利用者作成 settings / tags / collections の削除・defaults 上書きは承認されていない。

**detached / viewport 構造合意が必要な箇所:**

1. first-present hook は `vendor/eframe/src/native/wgpu_integration.rs:877` と共通
   `vendor/egui-wgpu/src/winit.rs:479` に触れる。root-only event を足すが、同 painter の deferred/immediate viewport が通る。
   paint skip / texture delta の既存契約を変えず、root milestone を detached paint で確定しない構造変更。
2. Directory prepared-scan を使う loader は `src/app.rs:24695` / `:25655` の shared path。
   context-owned folder pending と history/navigation owner、§1.335 intent を保持する必要がある。
3. 明示 startup file の既存 fullscreen continuation は detached host を作り得る。
   `src/app/startup_ops.rs:874` 以降には bookmark の detached predicate 群もある。
   初期/default の main owner を明示し、共有 helper の変更が sibling branch に及ばないか review する。
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
| A: timeline process owner / writer | 共通小 crate、launcher/core entry、全 stage、watchdog | fake monotonic clock で begin/end・error・skipped・parent duration、5/15/30秒各一回、heartbeat が動いても発火、stage切替/reset、並行 lane、overflow/drop、writer blocked/failed でも publisher 完了、UNC/extended UNC/mapped/unknown、env 上限/不正/直接core/同run相関。遅い sink は latch で止め、UI側が待たないことを実際の API で確認 |
| B: root presenter の証拠 owner | vendor の observer / frame tag 接続、outer first-update | fake paint outcome: surface absent/timeout/recreated/zero-size/skip/submit-only は成功なし、present-return だけ一度、root以外なし、Overlay→NormalShell、multi-pass/discard の最終tag、first-updateだけでreadyにならない。既存 texture-delivery / font-atlas / surface regression を保持 |
| C: App-global IndexerInit owner | enum、全consumer、単一採用、overlay/status | 実 receiver と injected spawn で4.999秒/5秒/期限同時Ready、恒久Emptyでも通常UIへ、30秒Ready後一度採用、Unavailable/Failed/disconnect/spawn失敗一度通知、poll early-return/idle、Pending中のfull-check集約、favorite ON/OFF/root/PDF password変更後に最新configuration採用、speed変更は次回起動反映のまま、similar bootstrap非早期閉鎖、close前late-result、manager/writer生成数1 |
| D: 既存 initial navigation request owner | target snapshot、defer until present、resolve+pre-scan、async adopt、§1.335 compose | fake resolver/scanを停止して先にNormal root present・navigation可を確認。再navigation/activation/Remote取得/closeでstale結果非採用・旧trace非再活性化、modal-held保持、scan errorと空を区別、default fallbackは一度。実temp directory/ZIP/PDF/変換fixtureでtarget・cursor・source・prefix・auto-open保持 |
| E: pure UI renderer | Japanese overlay / persistent status | fixed clock snapshot: light/dark/strong contrast、480×360、小viewport、長いstage名/999秒、Overlay/Pending/Unavailable。glyph check zero。first_setup snapshotを更新せず別枝責任 |

folder 回帰 matrix は Desktop / Previous / Specific / Drives / ReadingHistory、missing child→ancestor、
missing drive→Desktop、network/OneDrive解決停止、カーソル同一場所だけ、argv file / ZIP / PDF / EPUB /
converted archive、password取消、Ignore/Refused、Remote fallback を含める。
§1.335 統合後は parent list→book→quit→Previous、明示book listへの復帰、ZIP内部prefixとBackspace、
Drive sentinel、new record migration、直接読書、A/B、tray resume の同枝回帰を再利用・追加する。
multi-context は main のstartup scan待ち中に independent/parked viewer を保持し、
items/generation/channel/cancel/cache/restore recordが siblingに及ばないことを実handlerで確認する。
headless試験のnormal-presentは実painter証拠の代替ではなく、同一adoption APIに fake outcomeを注入する。

実装順は A → B → C → D → E、各chunkの終了で関連試験と独立review。
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

利用者への必須の追加情報質問は現時点ではない。レビューで決める提案値は
overlay 5秒、watchdog 5/15/30秒、診断1024件/24KiB handoff、2MiB/file・10run保持。
稀なdiagnostics保存不能は「一度通知し次回起動」、Indexer spawn/errorは検索Unavailable・閲覧継続・再起動を提案する。
データ損失を伴わず、retry機構を増やさない。設計leadがこの割り切りを利用者判断と照合し、
より複雑な復旧が必要なら実装前に相談する。画面前の同期停止まで正常UI保証が必要という判断なら、
今回のscopeを拡張せず、上記launcher監視か必須初期化の背景化を別のcoherent phaseとして相談する。
