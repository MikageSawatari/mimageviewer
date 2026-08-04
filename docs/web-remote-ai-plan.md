# mIV Remote 段 3b — AI アップスケール / デノイズ設計

状態: **3b-0・3b-1a・3b-1b・3b-2 実装済み、3b-2 は実機確認待ち**
（2026-08-04）。

親計画: [web-remote-left-panel-plan.md](web-remote-left-panel-plan.md)

正本: [display-pipeline.md](display-pipeline.md) /
[async-architecture.md](async-architecture.md) /
[preset-and-adjustment.md](preset-and-adjustment.md) /
[ai-processing-size-threshold-plan.md](ai-processing-size-threshold-plan.md)

## 1. 前提と今回の訂正

### 1.1 リモート接続中は PC がモーダルで停止している

これは段 3b の設計前提であり、以後の変更でも崩してはならない。

- `show_remote_session_dialog` は `egui::Modal` で「リモート接続中」を表示する。
- `remote_session_active()` は `common_modal_dialog_open()` に含まれる。
- `common_modal_dialog_open()` は、背面の main UI と fullscreen 入力を止める共通述語である。

したがって、通常のリモート接続中に PC 利用者がページをめくり、PC の新しい表示要求が
remote AI と競合する状況は起きない。旧版 §5 の LocalForeground / RemoteForeground lane、
PC 優先 preemption、tile 境界での協調取消し、remote job の最初からの再開、
`PREFETCH_IDLE_THRESHOLD` 待ちは、存在しない操作競合を前提にしていたため**すべて削除する**。

同じ誤りを繰り返さないため、設計レビューでは最初に
「リモート接続中は PC がモーダルで停止している」を確認する。

### 1.2 ただし接続取得時の残存ローカル AI はある

現在の `pause_local_progress_for_remote_session()` が止めるのは media、slideshow、
animation、continuous navigation であり、`final_ai_pending` や `AiJobQueue` は取消していない。
さらに `App::update` と active detached context の更新は、モーダル中も既存結果を poll し、
開いていた fullscreen page に対する `prefetch_final_ai()` まで呼び得る。

つまり PC 利用者の**新しい操作**は発生しないが、接続前から残っていた local final AI、
prefetch、ほかの `AiRuntime` 利用 worker は接続取得後もしばらく動き得る。
これは優先度や preemption ではなく、§4 の**接続取得 barrier**で一度だけ静止化する。
remote AI の inference dispatch はその完了後に始める。

## 2. 目的、不変条件、非目標

段 3b は、PC と同じ AI アップスケール / デノイズ設定を remote から保存し、
remote の静止画にも本体と同じ順序・model・size 規則で適用する。

不変条件:

- AI の正本は本体の `AdjustParams`、`AiRuntime`、model 選択、AI size 上限とする。
- 適用順は 色調補正 → final AI → smart sharpen → colorize → Creative LUT →
  post-filter のまま変えない。
- remote が操作権を持つ間と、その取得・解放処理中は PC 入力を再開しない。
- local と remote の GPU inference は接続 lifecycle の barrier で直列化する。
- UI thread は model load、inference、decode、edit materialize、WebP encode、worker join を行わない。
- 実行時の空き VRAM / RAM によって AI の適用可否や結果を変えない。
- remote のために別のアップスケール / デノイズ規則を作らない。

非目標:

- 動画 grade、動画アップスケール
- AI algorithm、model、tile 寸法、size 上限の変更
- PC の local AI 操作 UI の変更
- process 再起動をまたぐ job 永続化
- local / remote の同時要求を同一 inference へ合流させる共有 result cache

## 3. 所有権と受け渡し

### 3.1 現状の制約

- `RemoteIpcServer` は `eframe::run_native` より前に作られ、`App` は creator closure 内で
  後から作られる。`SessionHandle` が server と `App` の既存の受け渡し点である。
- 永続書き込みは `SessionHandle` の型付き bounded queue と repaint wakeup を経由し、
  `App` の UI thread が所有状態を変更する。
- `AiJobQueue` は worker 1 本で、local の Display は LIFO、Prefetch は FIFO である。
  `AiRuntime` の sessions mutex が inference を直列化するため、worker 数を増やしても
  final AI の throughput は増えない。
- `AiJob` は `Arc<ColorImage>` を持つため、inference 自体に idx は不要である。一方、
  `FinalAiKey`、pending、cache、結果採用は local の idx と表示世代を前提にする。
- cancel は既存の tile 境界で観測される。1 回の ONNX `Session::run` と model load の途中は
  外から停止できない。

### 3.2 案の比較と推奨

| 案 | 内容 | 判断 |
| --- | --- | --- |
| A | remote worker が別 `AiRuntime` を作って直接 inference | model session と常駐 VRAM が重複し、切断 drain の所有者も二つになるため不採用 |
| B | `App` の `Arc<AiRuntime>` だけを remote worker へ公開 | GPU は直列化できるが、受付、取消、drain 完了、stale result 採用を `App` が統制できないため不採用 |
| **C** | **`App` が install する型付き `RemoteAiExecutionBridge` を通す** | **singleton Runtime、worker 上の重処理、session generation と drain の一元管理を両立するため推奨** |

推奨案 C でも、秒単位の処理を UI thread で実行しない。source preparation、model load、
inference、final composite、encode は worker で行い、状態遷移だけを typed message と
repaint wakeup で `App` へ返す。

`App` が持つのは acquire / drain barrier と job lease の会計だけである。effective params と
snapshot の取得・再検証は worker 側に置く。remote の live settings / edit DB 取得は
`ContainerEngine` worker が所有しており、検証のために UI thread へ引き戻すと DB 読み取りを
UI thread へ持ち込むことになる。これは §2 の「UI thread は decode / edit materialize /
worker join を行わない」と衝突する。旧版はここを「UI thread が effective params を検証」と
書いていたが、実コードの所有境界と合わないため訂正した (2026-08-04)。

既存 local `AiJobQueue` の Display LIFO / Prefetch FIFO は変更しない。remote のための
priority lane は追加せず、接続取得後は local producer を止めて remote job だけを admission する。
remote は現在の page group 一つだけを持ち、新しい group / 設定が来た場合だけ
旧 remote job を `Superseded` として置き換える。

### 3.3 3b-0 実装時の local producer / Runtime consumer inventory

2026-08-04 の source inventory は次のとおり。acquire barrier は「UI から起動できるか」
ではなく、remote modal の背面でも worker が残り得るかで対象を決める。

| producer / consumer | owner と pending | 3b-0 barrier |
| --- | --- | --- |
| final upscale / denoise | App-global AiJobQueue、viewer-context 別 final_ai_pending、App-global retained orphan | queue 内 token と main / active-detached の pending token を cancel。job が App-global activity lease を終端まで保持するため、park 済み bundle から pending が外れた worker も含めて待つ。Display LIFO / Prefetch FIFO 自体は変更しない |
| legacy upscale / denoise | App-global ai_upscale_pending（段階削除待ちの旧経路） | producer gate、全 token cancel、pending 0 待ち |
| erase MI-GAN preview / commit | viewer-context 別 erase_inpaint_pending | producer gate、main / active-detached の token cancel。worker activity lease により park 時に pending map から外れた job も終端まで待つ |
| 保存済み erase を含む製本 composite | App-global book_op_pending が BookEraseRunner 内で Runtime を使い得る | cancel 単位が無いため worker の terminal result まで待つ |
| local-adjust subject segmentation (BiRefNet) | App-global local_adjust_segmentation_pending | 新規起動を gate。現行 worker に cancel token が無いため terminal result まで待つ。region segmentation は同じ pending slot だが CPU-only |
| video upscale | App-global video_upscale_running、pause / paused_idle | acquire で pause、paused_idle まで待つ。remote release 後、取得前に利用者が pause していなかった場合だけ再開 |
| model / backend 管理 UI | preferences、TRT worker notice、editing pack 更新時の unload_model | inference producer ではなく modal 背面から新規起動しない。接続前から残る TRT worker 起動は activity / in-flight owner の終端まで待つ。Runtime owner は同じ App singleton |
| remote 保存済み erase | ContainerEngine の Page operation | 旧 duplicate runtime を撤去し、App が install した RemoteAiExecutionBridge から Runtime / ModelManager を取得。Page operation token が drain accounting と render cancel を兼ねる |

edit_source.rs と video/upscale/job.rs は上記 producer から渡された Runtime を消費する純実行側で、
独立した admission owner ではない。heuristic classification、region segmentation、colorize は
AiRuntime consumer ではない。

## 4. 接続取得・切断 lifecycle

### 4.1 一つの型付き session phase

新たな bool を足して状態を分散させず、操作権を次の一つの phase で表す。

    Local
      → AcquiringRemote
      → RemoteActive
      → DrainingRemote
      → Local

`AcquiringRemote` と `DrainingRemote` も PC 入力を止める状態である。
現在の `remote_session_active()` 相当の modal predicate は三つの remote phase をすべて含める。
modal を閉じる条件と local AI producer を再開する条件は、どちらも `Local` への遷移だけとする。

3b-0 で旧述語の全 8 consumer を個別に監査した。いずれも「remote request を実行できるか」
ではなく「PC 側の入力・自動進行を止めるか」を問う箇所だったため、三 phase 共通の
`remote_session_blocks_local_control()` へ移した。

| consumer | phase の意味 |
| --- | --- |
| `common_modal_dialog_open()` | main / fullscreen の背面入力を三 phase すべてで止める |
| gamepad dispatch | window focus を迂回する PC 入力を三 phase すべてで止める |
| native video output event | ParkedLive の PC 側 event 適用を三 phase すべてで止める |
| main-focus fullscreen reconciliation | acquire / drain 中も viewer を閉じない |
| video open autoplay | acquire / drain 中の自動再生を始めない |
| audio open autoplay | acquire / drain 中の自動再生を始めない |
| animation playback construction | acquire / drain 中のアニメーションを始めない |
| passive detached activation | acquire / drain 中に PC click で viewer owner を移さない |

`RemoteActive` だけを意味する既存 consumer は無かった。3b-1 の remote AI admission 用には
別の `remote_session_operational()` を用意し、local input block と同じ述語へ混ぜない。

### 4.2 接続取得 barrier

remote client が操作権を取得したら、次の順で remote AI を開始可能にする。

1. phase を `AcquiringRemote` にし、PC の modal を先に開く。
2. main / detached / fullscreen の全 local AI producer を admission 停止する。
3. queued local prefetch / display job を cancel し、既存の in-flight local inference、
   model load、result finalize が終端を返すまで非同期に待つ。
4. `AiRuntime` を使う final AI 以外の local worker も inventory し、同じ barrier へ参加させる。
   少なくとも erase / MI-GAN、subject segmentation、video upscale の既存 in-flight を確認する。
5. local 所有の inference が 0 になってから `RemoteActive` とし、待機中の remote AI を dispatch する。

待機中も UI thread は block しない。PC 側は「リモート接続の準備中」、スマホ側は
`AcquiringRemote` 中の POST は job 登録まで受け付け、`WaitingForLocalDrain` /
「PC 側で開始済みの AI 処理の終了を待っています」と表示する。
ここで待つのは接続時の一回だけであり、接続中の PC 優先 scheduling ではない。
取消不能な model load や inference call はその一回が完了するまで待つ。

### 4.3 切断 barrier

現在の `local_disconnect()` は即座に `release(Local)` し、active session を外して
`control_return_sequence` を進める。このままでは modal が先に閉じ、remote worker が
残った状態で PC が再開する。段 3b では次の二段階へ変更する。

1. PC または remote の明示切断を受けたら `DrainingRemote` へ遷移する。
2. 新しい remote request の admission を閉じ、`session_closing` を直ちに返す。
3. session generation に属する queued source / AI / composite / encode / video job を取り除き、
   各 request の reply を terminal error で完了する。
4. in-flight job の cancel token を立て、worker が `Cancelled` または別の terminal result を
   返すまで待つ。現在の tile、ONNX call、model load の非割込み単位までは完了し得る。
5. `SessionHandle` の未 claim UI request は破棄して明示 error を返す。すでに claim され、
   DB transaction を開始した永続書き込みは途中で捨てず、成功または失敗の確定まで待つ。
6. session 所有の queued / claimed / in-flight count がすべて 0 になってから、初めて
   session を final release し `control_return_sequence` を一度だけ進める。
7. `poll_remote_session()` がその sequence を観測して
   `reload_after_remote_session_release()` を実行する。その後に modal が閉じ、PC が再開する。

reload は remote の確定済み DB 書き込みを読む一方、取消中の job や stale result と競合しない。
`reload_after_remote_session_release()` を drain 開始時へ前倒ししてはならない。

final release の成否は typed lifecycle の `DrainingRemote -> Local` 遷移だけを正本とする。
`active: Option<_>` は解放対象の payload であり、遷移可否の sentinel にしてはならない。
payload が欠落していても最終遷移と `control_return_sequence` の一度だけの前進は完了し、
欠落または不正遷移は release build でも invariant violation として記録する。状態遷移を
`debug_assert!` の評価式に置くことは禁止する。

別 client の takeover も旧 owner の即時置換にはしない。旧 session の drain 中は新 client へ
`session_closing` と retry hint を返し、final release 後に改めて acquire させる。

### 4.4 drain が長いとき

PC の modal は閉じず、見出しを「リモート接続を終了しています」に変え、次を表示する。

- queued / running の残数
- 「AI tile の終了待ち」「model 準備の終了待ち」「保存の確定待ち」の現在 phase
- 経過時間

一定時間後は「処理の安全な終了を待っています」と説明を追加するが、modal だけを
強制的に閉じる button は設けない。worker hang は別障害として記録し、
drain timeout 後に PC を再開して同時実行する fallback は作らない。

## 5. job の同一性

既存 `FinalAiKey` は idx + `EditResultKey` を含み、local cache と表示世代の検証に適している。
remote のためにこれを path key へ置き換えず、次を持つ `RemoteFinalAiKey` を新設する。

- canonical page key と source revision
- erase / local-adjust / conceal を含む edit snapshot fingerprint
- AI 入力 pixels の width / height
- `color_ai_hash` 相当の effective params と background mode
- AI size limit、model-pack / runtime epoch、pipeline schema version

remote の `target_px` は AI key に含めない。本体と同じ canonical / native input に inference し、
remote 表示寸法への縮小は AI 後の output adapter で行う。PDF も本体と同じ判定に従う。
`job_id` は client id、session generation、単調 sequence と結び付ける。同じ request id の再送と、
同じ active remote group の exact key は一つへ coalesce する。

PC と remote は同時に表示要求を出せないため、旧設計の subscriber fan-out と
local 優先 completed-work pool は作らない。model session と `AiRuntime` は共有するが、
local cache と remote bounded cache は分離する。disconnect 後に PC が同じ page・設定を表示し、
local cache miss なら再 inference する。この逐次 2 回目が実測で問題になった場合だけ、
`RetainedFinalAiKey` 互換の exact promotion を別段で設計する。

## 6. HTTP、状態、進捗

| HTTP | 用途 | 成功 |
| --- | --- | --- |
| `POST /api/ai/jobs` | 現在 page group を effective AI 設定で開始 | `202` + `job_id` |
| `GET /api/ai/jobs/{job_id}` | 状態取得 | active / retained terminal は `200` |
| `GET /api/ai/jobs?recoverable=1` | 同じ client の復帰可能 job を列挙 | `200` |
| `DELETE /api/ai/jobs/{job_id}` | job 一件の明示取消 | `202`、terminal なら冪等に `200` |

POST は inference 完了を待たない。UI request accept timeout の 2 秒以内に `App` が受付できなければ
late start を禁止する。`AcquiringRemote` では `WaitingForLocalDrain` の job を受理してよいが、
`DrainingRemote` 以降の新規 POST は `409 session_closing` とする。
AI job 自体には、本体にない固定の計算 deadline を設けない。

状態は `WaitingForLocalDrain`、`PreparingSource`、`LoadingModel`、`Denoising`、
`Upscaling`、`Finalizing`、`Cancelling`、`Ready`、`Superseded`、
`CancelledByUser`、`DiscardedByHost`、`BackgroundExpired`、`Failed` とする。

総合 percent は推測せず、model load / finalize は indeterminate、denoise / upscale は
`completed_tiles / total_tiles` を返す。見開きは page index / count、両 stage 使用時は
stage index / count も返す。algorithm は PC と共有し、既存 tile loop に軽量 progress sink を足す。

polling は foreground nonterminal で 500 ms、foreground 復帰時は即時、一時通信失敗は
1 s → 2 s → 5 s 上限の backoff とする。background では polling を止め、
browser timer を keepalive にしない。旧設計の `WaitingForLocal` や「PC で閲覧中」は存在しない。

## 7. スマホ側の terminal な終わり方

PC 側で切断開始した時点で remote job registry を `Cancelling` にし、スマホへ
「PC 側で接続が終了されたため AI 処理を中止しました」を通知する。スマホはこの時点で
通常の spinner を終え、黙って成功を待ち続けない。PC 側の内部 drain はその後も続く。

drain 完了後は `DiscardedByHost` を 10 分保持する。session が inactive でも同じ認証済み client は
terminal metadata を取得でき、保持期間後は `410 job_gone` と terminal reason を返す。

- PC の切断: `DiscardedByHost`。自動再開しない。
- スマホの job 取消: `CancelledByUser`。session 自体は継続できる。
- 新 client への置換: 旧 client は `Superseded`。自動再開しない。
- core 終了 / 再起動: 「本体が再起動しました」と表示し、勝手に再送しない。

outstanding HTTP / IPC request にも channel close だけでなく同じ terminal code を返し、
画面上の job と request 単位の error を一致させる。

## 8. 画面消灯と復帰

画面消灯は明示切断ではない。AI を動かしたまま session を release すると PC が再開して競合するため、
nonterminal AI job がある時だけ liveness lifecycle を拡張する。

1. browser background では polling を止める。HTTP connection の有無は job ownership にしない。
2. 現在 60 秒の liveness timeout に達しても nonterminal job があれば即 release せず、
   `RemoteActive/ClientDetached` として PC modal を維持する。
3. inference は継続し、同じ client が戻れば recoverable GET で同じ job を復元する。
4. background 中に job が terminal になったら結果と metadata を 10 分保持する。
   remote 所有作業が 0 なので session は安全に release でき、PC を再開してよい。
5. 最終 activity から既存 `IDLE_TIMEOUT` と同じ 10 分を超えても nonterminal なら、
   `BackgroundExpired` として cancel / drain し、その完了後に PC を再開する。
6. nonterminal job が無い通常の liveness timeout は現行どおり release してよい。

これで「画面消灯は復帰する / 明示切断は復帰しない」を terminal reason で区別する。
10 分は unattended ownership の上限であり、foreground の AI 計算 deadline ではない。
PC modal には「スマホは一時停止中です。AI 処理は継続しています」と表示し、
PC 利用者が明示切断を選んだ場合は §4.3 の drain を通す。

## 9. 重複 `AiRuntime` と MI-GAN

`ContainerEngine::remote_inpaint_runtime()` は保存済み消しゴム用に別 `AiRuntime` を遅延生成する。
一方 `AiRuntime` の契約はアプリ全体で一つを `Arc` 共有する形である。

PC 優先 preemption は統合理由ではなくなったが、統合は引き続き **3b の前提作業**とする。

1. App runtime に local model が残った状態で remote runtime が同じ model を load すると、
   model session と常駐 VRAM が二重になる。
2. 別 runtime / worker が残ると、切断 barrier が remote GPU work 0 を一つの owner から確認できない。
3. 入力規則と backend / model lifecycle を二つの実体へ同期する保守負債が残る。

別 runtime 維持は不採用、raw `Arc<AiRuntime>` だけの公開も drain accounting を迂回するため不採用。
3b-0 で `ContainerEngine::inpaint_runtime` を撤去して remote MI-GAN を typed bridge へ移す。
3b-1 の remote final AI も同じ bridge / executor の逐次 job とする。local の全 AI 機能を
新 queue へ移す必要はないが、
接続取得 barrier の対象を漏らさないため `AiRuntime` 利用箇所を inventory する。

3b-0 ではこの前提作業を実施済み。ContainerEngine は Runtime / ModelManager を所有せず、
server 起動後に App が SessionHandle へ install する RemoteAiExecutionBridge を参照する。
App runtime が既にあれば同じ Arc を publish し、未生成なら remote Page worker 上で bridge が
DirectML Runtime を一度だけ遅延生成する。App は ready Runtime を非同期 poll で adopt するため、
model load / Runtime 初期化を UI thread へ移していない。

## 10. VRAM / RAM

`src/app/vram_accounting.rs` は egui `TextureHandle` を数えるため、ONNX Runtime の
session / tensor / provider allocation と CPU `ColorImage` は会計に入らない。
remote 最終画像も WebP であり egui texture へ upload しない。

推論領域を texture 相当として擬似加算せず、必要なら入力寸法、tile 数、backend、model、
elapsed、CPU buffer 概算を観測専用 perf event にする。値を admission には使わない。

acquire / disconnect barrier により local と remote の同時 inference 数は 1 とし、
共有 `AiRuntime` で同一 model session の重複 load を避ける。remote cache は固定件数 / MiB 上限で
local cache を evict しない。適用判断と上限は設定だけで決め、空き VRAM / RAM へ適応させない。
高負荷設定での OOM は既存方針どおり許容する。

## 11. 再利用するもの / 新設するもの

再利用:

- `RemoteWriteRequest::SetAdjustment`、既存 scope、undo、cache 差分無効化
- `SessionHandle` の bounded queue、dispatch cancel、repaint wakeup
- App の `AiRuntime` / `ModelManager` と local `AiJobQueue` の既存規則
- cancel token、tile 境界、model / size / PDF 判定
- `FinalCompositePlan` / `execute_final_composite`
- remote の decode / materialize / composite / WebP adapter
- final release 後の `reload_after_remote_session_release()`

新設または一般化:

- typed session phase、acquire barrier、disconnect drain coordinator
- `RemoteAiExecutionBridge` と remote typed AI / MI-GAN job
- remote admission 停止、queued purge、in-flight 終端 acknowledgment
- `RemoteFinalAiKey`、job registry、HTTP start / state / cancel / recoverable
- terminal metadata / result の 10 分保持、`ClientDetached`
- tile progress sink と phase telemetry

新設しない:

- local / remote priority lane
- active remote soft-preemption と cancel + 自動 restart
- local / remote subscriber fan-out と共有 completed-work pool

## 12. 撤退条件

次のいずれかを満たしたら、remote AI 実行は出荷しない。

1. acquire 時に全 local `AiRuntime` job の admission 停止と終端確認ができない。
2. disconnect 時に remote work を列挙できず、modal close 後に GPU work / stale result が残り得る。
3. 別 `AiRuntime` または重複 model session / VRAM を残さないと実装できない。
4. remote だけ model、tile、size 上限、適用順、PDF 判定を変える必要が出る。
5. disconnect / background / superseded の terminal identity を保持できず、スマホが待ち続け得る。
6. source / edit / session generation の stale result を別 page へ公開する可能性を閉じられない。
7. model load、inference、decode、encode、drain wait を UI thread で同期実行しなければ成立しない。
8. AI を使わない remote 接続・video streaming・release 後 reload の lifecycle を保てない。

旧条件 1「remote 中の PC LocalForeground の待ち」と旧条件 2「cold model load が PC を block」は削除した。
model load が長い場合は両 UI へ phase を表示するが、それ自体は撤退条件にしない。

撤退時は段 0〜3a と AI 読み取り専用表示を残し、remote AI UI / job API / result cache を捨てる。
3b-0 の singleton Runtime / phase 整理は無挙動変更を証明できた場合だけ残す。
AI 設定だけを書けて remote の絵には AI が乗らない状態は出荷しない。

## 13. 実装する場合の分割

旧版の scheduler / preemption / shared cache 段が不要になったため、4 段から 3 段へ減らす。

### 3b-0: 排他 lifecycle と singleton Runtime

- typed phase、acquire barrier、disconnect barrier を純状態機械で実装する
- local AI producer / `AiRuntime` consumer を inventory する
- duplicate remote `inpaint_runtime` を App 所有 typed bridge へ統合する
- drain 中も modal を維持し、final release 後だけ reload することを固定する
- この段では remote AI UI と real final AI job を出さない

**実装済み (2026-08-04):** 上記 lifecycle、barrier、inventory、singleton bridge と
final-release 順序を実装。この段の時点では未実装だった remote final AI job / job protocol は
3b-1b で接続済み。remote AI UI は 3b-2 のまま。

### 3b-1a: 共有 canonical decoder（完了）

- fullscreen の通常画像 / verified bytes / ZIP・nested ZIP を同じ typed source decoder へ集約する
- GIF/APNG/WebP の現行 Animated 分類、image crate → WIC → Susie、EXIF 適用を共有する
- panorama は clamp 前 native image を従来位置で tee し、通常 static だけ 8192 clamp する
- raster PDF は既知 content-type snapshot から native 長辺（最大 8192）で render し、
  vector は render しない canonical API を用意する
- この段では remote / job protocol へ接続しない。本体 fullscreen の無挙動変更を先に
  自動 test と実機 smoke で確認する

**実装状況 (2026-08-04):** `7b8b31a1`。コードと自動 test に加え、通常画像 / EXIF 回転 /
ZIP / ZIP 内 GIF / animated GIF・APNG・WebP / panorama / 8192 超の実機 fullscreen smoke まで完了。

### 3b-1b: job protocol と end-to-end backend（完了）

- fake executor で start / state / cancel / disconnect / background recovery を固定する
- source、MI-GAN、final AI、final composite、WebP を bridge へ接続する
- 通常画像、ZIP、nested archive、raster PDF、製本 page の canonical input を揃える
- vector PDF、size gate、stale result 拒否、terminal 10 分保持を実装する

**実装状況 (2026-08-04):** backend と protocol を実装。job ごとに `SessionOperation` を保持して
disconnect drain へ参加し、参照 IPC は session operation を増やさない。client presence は
foreground / detached を型で分け、background 復帰・10 分 expiry・terminal/result 10 分保持を
fake executor test で固定した。AI cache key と公開 result identity は分離し、公開直前に source /
edit / settings snapshot を worker で再取得して不一致を stale とする。native AI cache は local cache
と eviction を共有しない専用 LRU だが、上限値は同じ既存設定
`retained_final_ai_cache_max_entries` / `retained_final_ai_cache_max_mib` をそのまま使用し、0 は無効とする。
result は Finalizing で page ごとに一度だけ縮小・WebP encode し、保持済み bytes を GET へ返す。
vector PDF / size gate は runtime 取得前に typed `not_applicable` へ終端し、nested ZIP は remote
source decoder から共有 canonical decoder へ到達することを focused test で直接固定した。

### 3b-2: Web UI

- AI model 選択を `SetAdjustment` へ接続する
- phase / tile 進捗、取消、acquire 待ち、disconnect error、画面消灯復帰を表示する
- 実機で接続直前の local AI、AI 中の PC 切断、画面消灯復帰を確認する

**実装済み (2026-08-04):** `RemoteAdjustmentValues` の省略可能な typed `ai` と、
`AiFeatureMode` の選択可否を含む server-owned model catalog を追加した。旧 SPA の `ai` 欠落は
AI 値を変更しない。SPA は `/api/page` の表示完了後に現在の 1〜2 ページだけを自動開始し、
foreground では 500 ms、通信失敗時は 1 s → 2 s → 5 s で state を取得する。background では
timer を停止し、復帰時は session 再取得後に recoverable を先に照会する。進捗は phase と server の
page / stage / tile counter だけを表示し、percent は合成しない。aggregate `Ready` の page outcome が
`Ready` のページだけ result を取得し、全 result を decode 後に見開き DOM を一度だけ更新する。
`NotApplicable` は元画像を保ち、失敗表示にしない。

## 14. 必須 test / 計測

- acquire で PC modal が先に開き、main / fullscreen / detached 入力が止まる
- queued local prefetch を捨て、in-flight local AI の終端前に remote AI を開始しない
- `RemoteActive` 中に local `prefetch_final_ai()` が inference を admission しない
- disconnect 後の新規 request は `session_closing` となり late start しない
- queued remote は明示 error、claimed 永続書き込みは確定後に drain 完了になる
- active tile / model load の終端まで modal と PC input block を維持する
- work count 0 → final release → `control_return_sequence` → reload の順を固定する
- PC 切断をスマホが `DiscardedByHost` と表示し spinner を継続しない
- background polling 停止後も同 client が同じ job を復元できる
- background expiry は drain 後だけ PC を再開し `BackgroundExpired` を保持する
- remote MI-GAN / final AI は同じ Runtime owner を通り、同時 inference 数は 1
- vector PDF は AI を起動せず、raster PDF は本体と同じ native input へ収束する
- 空き VRAM 値を変えても適用判断と key が変わらない
- perf は acquire local-drain、remote inference、disconnect drain を分離記録する
