# TensorRT worker lifecycle owner 設計 (§1.243)

状態: **実装・focused / full / static gate・独立 completion review・unsigned release verification build・限定隔離実機5項目完了** (2026-09-15)。通知入力の別観測は§13を参照。

この文書は [next-release-backlog.md](next-release-backlog.md) §1.243 の正本である。
§1.241 の ORT / VC Runtime 修正を入れた GPU 実機検証で見つかった、決定的な
TensorRT worker 起動失敗を画像ごとに繰り返す問題を、worker の所有境界で直す。
v4.0.0 出荷前の必須修正とし、§1.241 の成功済み証拠は変更しない。

## 1. 観測、期待、不具合の境界

### 1.1 実測

ClaudeCode が RTX 4090 の隔離 portable profile で TensorRT pack の
`onnxruntime.dll` を同じ長さのまま壊し、AI upscale 対象を 6 枚表示した。
`target/section241-release-verification-20260915/gpu/gui-4-tensorrt-packort-corrupt.log`
には次の起動時刻が残る。

| lazy start | typed failure | 経過 |
| --- | --- | --- |
| 8.258 s | 8.289 s `RuntimeInit` | 31 ms |
| 10.665 s | 10.695 s `RuntimeInit` | 30 ms |
| 12.645 s | 12.673 s `RuntimeInit` | 28 ms |
| 14.622 s | 14.649 s `RuntimeInit` | 27 ms |
| 16.581 s | 16.609 s `RuntimeInit` | 28 ms |
| 18.543 s | 18.571 s `RuntimeInit` | 28 ms |

各画像は DirectML fallback で処理でき、45 秒 timeout や GUI freeze は無い。
`WorkerStartFailureKind::RuntimeInit` も protocol から UI notice まで保たれている。
§1.241 の修正はここまで正しく、問題はその後の親 process の lifecycle である。

### 1.2 期待する不変条件

1. `RuntimeInit` / `CommandRejected` / `Protocol` / `ParentSetup` のような決定的失敗は、
   process 内で `Failed` terminal になる。同じ画像の次 tile、次画像、prefetch、book / materializer、
   video upscale、Remote request は子 process を再び起動せず DirectML を使う。
2. `Failed` を再び起動可能にするのは、backend の明示的な off → on、pack install 完了、
   通知の「ワーカーを再起動」、または app restart だけである。通知の「閉じる」は表示を消すだけで
   `Failed` を変えない。
3. 起動段階の一時的失敗 (`ProcessSpawn` / `Transport` / `Timeout`) は 2 秒後に 1 回だけ
   自動再試行する。推論中の worker death は従来どおり最初の 3 回を自動回復し、回復後に
   成功した inference が無いまま 4 回目に死んだとき terminal にする。
4. TensorRT が `Eligible` / `Starting` / `Failed` / `Disabled` の間も現在の要求は DirectML で完了する。
   SubjectMatte、MI-GAN、RealESRGAN General v3 は従来どおり常に in-process 経路を使う。
5. backend / pack / app lifetime が変わった後の遅い start completion を attach しない。
   旧 pool の遅い I/O failure で新 pool を detach しない。
6. UI thread では pack scan、process spawn / handshake、sleep、pool shutdown / wait を行わない。

### 1.3 現在の破れ方

`App::maybe_start_trt_worker_pool_for_ai_use` は設定、`AiRuntime::has_worker_pool()`、
`App::trt_restart_in_flight: AtomicBool` だけを見る。`TrtWorkerPool::start()` の失敗は
`AiRuntime::worker_notice` に一度載るが、guard は直ちに false へ戻る。したがって次の表示要求は
同じ lazy start を新規要求と判断する。型付き failure は UI 側の notice 自動再試行だけを止め、
lazy start の入口を止めない。

この分散には次の関連不整合もある。

- pool、spawn 中フラグ、spawn retry 回数、infer death retry 回数、notice が別々の owner にある。
- `report_worker_died` は報告した pool の identity を照合せず、現在の pool を無条件 detach する。
- pool を attach した時点で death retry 回数を 0 に戻すため、attach 後に最初の inference で死ぬ
  worker を繰り返すと「最大 3 回」が実質無制限になる。
- delayed retry は `sleep` 後に backend / pack / app lifetime を照合せず start / attach する。
- App の 2 つの lazy producer より下にある Remote、materializer、book、video producer は同じ
  upscale pipeline を使うが、worker start の判定は共有しない。
- `should_route_to_worker` と `infer_via_worker` が別々に pool を読み、1 画像の tile 間でも route が
  変わり得る。Remote cache digest も別の瞬間の bool を読む。

失敗 bool を追加しても、この非排他状態、非同期 completion、旧 pool ABA、producer の差は残る。

## 2. 現行 producer / consumer / lifecycle 棚卸し

| 種別 | 現在の入口 | 現在の worker 操作 | 修正後 |
| --- | --- | --- | --- |
| 通常 / fullscreen final AI | `App::maybe_start_final_ai` | App の lazy start | 共通 upscale request が owner の demand gate を 1 回通る |
| 旧 AI upscale / denoise と prefetch | `App::start_ai_upscale` 系 | App の lazy start | 同上 |
| book / stack / materializer | `books` / `materializer` → `final_pipeline` | start は行わず現在の pool だけ使用 | 同じ shared owner と demand gate |
| video upscale | `video/upscale/job.rs` → `ai::upscale::upscale` | start は行わず現在の pool だけ使用 | 同じ shared owner と demand gate |
| Remote AI | `RemoteAiExecutionBridge` → `remote_ipc/container` → final pipeline | start は行わず、digest は現在の pool bool | 同じ shared owner。digest は route generation を読む |
| backend Apply | `App::apply_ai_backend_change` | 即時 spawn / detach | owner の `select_backend`。on は revision 更新後に start、off は disable |
| pack install 完了 | `activate_tensorrt_after_install` | backend 保存後に Apply へ合流 | owner の `pack_installed`。同版再導入でも revision 更新して hot start |
| pack uninstall | `uninstall_trt_pack_now` | UI thread で detach 後、背景削除 | `PackUninstalling` と retire barrier を作り、全 lease / start / child shutdown 完了後だけ背景削除 |
| notice 自動回復 | `poll_trt_worker_notice` | App counter と guard で再 spawn | owner 内の typed policy と cancelable timer |
| notice 手動再起動 | `show_trt_worker_notice_dialog` | App counter reset 後 unguarded spawn | owner の `manual_restart` 一口だけ |
| app exit | `App::on_exit` | 明示 fencing 無し、最後の Arc drop 頼み | owner の `retire(AppExit)` で revision invalidation と背景 shutdown |
| Remote acquire barrier | `LocalAiRemoteBarrierSnapshot` | App の Atomic guard を読む | owner の `Starting` snapshot を読む |
| `bench_ai` | CLI が `TrtWorkerPool::start_with_exe` と direct attach | 同期 diagnostic start | product owner を迂回しない diagnostic 用 start / wait APIへ移す |
| infer / builder child | `--tensorrt-infer-worker` / `--tensorrt-build` | 自 process の ORT terminal | protocol と builder の既存契約を維持。GUI lifecycle stateには入れない |

`ai::denoise` は `ai::upscale` の tile pipeline を再利用する。通常表示、prefetch、book、
materializer、video、Remote は最終的にこの共通 pipeline へ入るため、App の呼出箇所を列挙して
個別に start するのではなく、この境界で route を取得する。

## 3. 単一 owner と型

### 3.1 配置

新しい `TrtWorkerLifecycleOwner` を `Arc` で process に 1 個だけ作る。
`AiRuntimeInitOwner::new()` が DirectML runtime より先にこれを作り、`AiRuntime` の生成時に同じ
`Arc` を渡す。App、Remote、materializer が共有する `Arc<AiRuntimeInitOwner>` から到達する
`AiRuntime` は必ず同じ TensorRT owner を見る。GUI process 内で別 owner、直接 pool attach、
直接 `TrtWorkerPool::start` を作らない。

App は settings load 後、DirectML runtime の Ready を待たずに要求 backend を owner へ設定できる。
app start で TensorRT が保存されている場合は `Eligible` にするが、現在の lazy startup を守り
子 process は最初の対象 AI demand まで起動しない。Remote が最初の demand でも同じ owner が起動する。

`bench_ai` のような独立 CLI は別 process の同じ lifecycle 型を明示的に configure し、terminal を
同期的に待てる diagnostic API を使う。製品 GUI の begin gate を迂回する public attach API は残さない。

### 3.2 排他 state

```rust
enum TrtWorkerState {
    Disabled {
        revision: TrtLifecycleRevision,
        reason: TrtDisabledReason,
    },
    Eligible {
        revision: TrtLifecycleRevision,
        budget: TrtRecoveryBudget,
    },
    Starting {
        ticket: TrtStartTicket,
        budget: TrtRecoveryBudget,
    },
    Attached {
        revision: TrtLifecycleRevision,
        pool_id: TrtWorkerPoolId,
        pool: Arc<TrtWorkerPool>,
        budget: TrtRecoveryBudget,
    },
    Failed {
        revision: TrtLifecycleRevision,
        failure: TrtLifecycleFailure,
    },
}
```

`TrtStartTicket` は少なくとも `revision`、単調増加 `attempt_id`、要求 backend、
`TrtStartCause` を持つ。cause は `BackendEnabled`、`PackInstalled`、`LazyDemand`、
`ManualRestart`、`StartupTransientRetry`、`InferDeathRetry` を区別する。

`TrtLifecycleFailure` は文字列ではなく次を保持する。

- `Start(WorkerStartError)`。§1.241 の `WorkerStartFailureKind` と detail / fallback reason を維持する。
- `DiedDuringInfer { pool_id, detail }`。
- `Internal(TrtLifecycleInternalFailure)`。owner task panic、background reaper の作成失敗など
  lifecycle 実装自身の破綻を、子 process の失敗と区別する。

OS が start task / child process thread を作れない実際の spawn error は既存の
`Start(ProcessSpawn)` として 1 回 retry できる。いったん実行を始めた lifecycle task の panic は
`Internal(TaskPanicked)` として直ちに terminal にし、同じ不具合を silent retry しない。

`Disabled` の reason は `BackendNotRequested`、`PackUnavailable(PackStatus)`、`PackUninstalling`、`AppRetired` を
区別する。TensorRT 設定だが Missing / Stale / Corrupt と判定できた pack は、各 demand で disk を
調べ直さず `PackUnavailable` に留める。pack install 完了か backend の明示的な再選択で再評価する。
`AppRetired` は absorbing state とし、終了後に到着した backend 選択、pack install、manual restart、
pack uninstall、AI demand のどれからも revision や task を作らない。

### 3.3 正本と projection

state の `Mutex` と revision が worker の起動可否、routing、retry budget の正本である。
次は正本にしない。

- UI banner の `Option<WorkerNotice>`
- notice を閉じたかどうか
- App の counter
- Atomic の spawn 中 flag
- pool field の `Some / None` 単独

owner は terminal / user-visible transition ごとに revision と紐付いた one-shot
`TrtWorkerEvent` を発行する。UI は event を表示用に consume するだけである。Close はその event の
表示を acknowledge するが、owner の `Failed` は残る。後着 consumer は event が消えていても
snapshot から起動不可と判断できる。

## 4. transition 契約

### 4.1 start と completion

1. 通常 AI producer は demand 専用 `route_or_begin(model_kind)` の同じ gate を通る。この API は
   caller から start cause や rearm 指示を受け取らない。
2. TRT 対象外 model、`Disabled`、`Starting`、`Failed` は DirectML route を返す。
3. `Eligible` は lock 内でただ 1 つの `Starting(ticket)` へ移り、owner 自身が
   `TrtStartCause::LazyDemand` を発行して start task を enqueue し、現要求へ DirectML route を返す。
   並行 demand は `Starting` を見て spawn しない。`Failed` の demand は常に DirectML を返し、
   rearm transition を表現する口を持たない。
4. start task は UI thread 外で最終 pack status を検査し、必要なら `TrtWorkerPool::start()` を行う。
5. completion は lock 内で current state が **同じ revision / attempt / requested backend の
   `Starting`** であるときだけ適用する。成功は新しい `pool_id` を割り当て `Attached`、失敗は
   typed policy に従って delayed retry または `Failed` にする。
6. stale success の pool は attach せず、background reaper へ渡す。stale failure は log だけで、
   current event / state / budget を変更しない。

pack status の I/O、worker process start と最大 45 秒の handshake、retry wait、pool shutdown / wait は
すべて owner の background executor で行う。UI の transition API は state 更新と command enqueue だけで
戻る。retry wait は `sleep` だけにせず revision / retire を wake できる timer とし、2 秒の間に backend
off、pack install、manual restart、app exit が来たら旧 ticket を終了する。

`select_backend`、`pack_installed`、`manual_restart` は rearm 権限を持つ別 API とする。
内部 retry は private completion / timer API だけが発行する。`TrtStartCause` の constructor と
revision increment は owner module から外へ公開せず、consumer が `LazyDemand` を `ManualRestart` 等に
偽装できないようにする。

### 4.2 明示 rearm

| event | revision | 次 state / 動作 |
| --- | --- | --- |
| app start、保存 backend=TensorRT | 新 owner の初期 revision | `Eligible`。lazy のまま |
| backend off | increment | `Disabled(BackendNotRequested)`。current / starting を invalidate |
| backend on / Apply | increment | `Eligible` から `BackendEnabled` start を即時 enqueue |
| pack install 成功 | increment | `Eligible` から `PackInstalled` start。既に同版でも新 install completion を識別 |
| manual restart | increment | budget と failure を reset し `ManualRestart` start を 1 回だけ enqueue |
| pack uninstall 開始 | increment | `Disabled(PackUninstalling)`。旧 revision の start / pool / route を retire group へ閉じる |
| pack uninstall 完了 | increment | 削除中に要求された backend が TensorRT なら `Eligible`、それ以外は `Disabled` |
| app exit | increment | `Disabled(AppRetired)`。timer / completion を invalidateし pool を背景 retire |
| notice Close | 変更なし | banner event だけ acknowledge、`Failed` 維持 |
| AI demand | 変更なし | `Eligible` だけ start。`Failed` は DirectML のみ |

backend off / uninstall は state を先に無効化してから pack delete 等を始める。各 revision は一つの
retire group を持ち、start task、attached pool、request route lease、reaper へ activity を一回だけ移譲する。
古い pool を state から外しただけで UI thread 上の最後の `Arc` を drop しない。最後の request lease が
解放した時点で pool を reaper へ送り、reaper が shutdown / `child.wait()` を完了して activity を閉じる。

pack uninstall は旧 revision の retire group を close した `TrtPackUninstallPermit` を背景削除 task へ渡す。
permit の barrier は、開始済みの pack scan / process start、stale success の child、attached pool、全 route
lease とその child shutdown が完了するまで開かない。削除は barrier 後かつ同じ uninstall revision が
current の場合だけ行う。削除中に来た backend 選択 / pack install は新しい start を作らず要求 backend
だけを更新し、削除完了後に `Eligible` へ戻す。install UI request も削除完了まで保持する。これにより
使用中 DLL の削除 race と、遅い削除が再 install 済み pack を消す race の両方を防ぐ。
すでに install dialog / worker が存在する間は uninstall 自体を開始しない。App の UI actor がこの入口と
install completion を直列化するため、atomic publish 済みの新 pack と `remove_dir_all` は並行しない。

### 4.3 retry policy

- 起動の `ProcessSpawn` / `Transport` / `Timeout` は同じ recovery episode 内で 1 回だけ、2 秒後に再試行する。
  2 回目の失敗、または `RuntimeInit` / `CommandRejected` / `Protocol` / `ParentSetup` は `Failed` terminal。
- `DiedDuringInfer` は、matching pool を外し、これまでに使った automatic infer recovery が 3 回未満なら
  silent restart する。death #1、#2、#3 はそれぞれ `InferDeathRetry` を 1 回 enqueue し、成功 inference が
  無いままの death #4 で `Failed` と user-visible event にする。
- death budget は pool attach では reset しない。新 pool で **実際の worker inference が 1 回成功**し、
  その `pool_id` がまだ current `Attached` なら reset する。attach 直後に死ぬ worker は有限回で止まる。
- manual restart、backend on、pack install は新 episode として両 budget を reset する。
- infer-death recovery 中の deterministic start failure はその場で `Failed`。transient start failure は
  当該 start episode の 1 回だけ再試行し、death budget を巻き戻さない。

## 5. inference route と pool identity

### 5.1 request 単位 route

`should_route_to_worker` と `infer_via_worker` の二段読みを、次のような一つの lease へ置き換える。

```rust
enum TrtInferenceRoute {
    DirectMl,
    Worker {
        revision: TrtLifecycleRevision,
        pool_id: TrtWorkerPoolId,
        pool: Arc<TrtWorkerPool>,
    },
}
```

`upscale_with_timings_impl` の入口で model kind を渡して route を 1 回取得する。tile size も全 tile の
実行先も同じ route から決める。途中で別 worker が attach しても DirectML から TensorRT へ切り替えない。
worker route が error になったら、同じ要求の mutable route を DirectML へ落として失敗 tile と残り
tile を処理する。I/O death の場合だけその pool identity を owner へ一度報告し、worker が正常応答した
model 固有 error なら shared state は `Attached` のままにする。これにより既存の same-tile fallback を
維持しながら、1要求の途中で worker を再利用して provider が往復することを防ぐ。

`report_worker_died(pool_id, detail)` は current state が同じ `Attached.pool_id` の場合だけ transition する。
旧 pool の遅延 failure は current pool を外さない。同じ pool の並行 failure は最初の 1 件だけが state と
budget を更新し、後続は dedup log だけにする。successful inference の報告も matching `pool_id` だけを受ける。

### 5.2 model routing

worker demand を発生させるのは現在の配布 engine 対象だけである。

- `UpscaleRealEsrganX4Plus`
- `UpscaleRealEsrganAnime6B`
- `UpscaleRealCugan4x`
- `UpscaleNmkdSiax4x`
- `DenoiseRealplksr`

`InpaintMiGan`、`SubjectMatte`、`UpscaleRealEsrGeneralV3` は owner が `Attached` でも DirectML であり、
これらだけの request は `Eligible` から start させない。`build_trt_pack` の required engine list と
同じ model predicate を共有または相互回帰する。

### 5.3 Remote

Remote は App と同じ runtime / lifecycle owner を使う。AI resources 取得後の共通 upscale entry が
最初の TRT demand になれるため、Remote 専用 start path は作らない。

Remote cache digest は `should_route_to_worker: bool` の瞬間値ではなく、owner の read-only
`TrtRouteGeneration` (`DirectMl` または `Worker { revision, pool_id }`) を含める。digest read は start を
起こさない。digest 後に route が変わった場合、その旧 generation の result は新 generation から参照
されない。

local → Remote control handoff の barrier は `trt_restart_in_flight: AtomicBool` を廃止し、owner snapshot の
`Starting` だけを blocker とする。`Eligible` / `Failed` / `Disabled` は background spawn が無いので blocker
ではない。Attached の inference 自体は既存の local activity / job owner が数える。

## 6. UI と利用者向け動作

`poll_trt_worker_notice` は retry policy を持たず、owner event を UI model へ写すだけにする。

- deterministic start failure、transient retry exhausted、infer death budget exhausted は現在と同じ
  常駐 banner と detail を出す。
- silent retry の開始 / 成功は log に残し、途中 banner は出さない。
- 「ワーカーを再起動」は current backend が TensorRT のとき owner の `manual_restart` を一度呼ぶ。
  accepted revision を banner と照合し、double click / stale banner から複数 start しない。
- 「閉じる」はその event を消すだけ。後続画像を表示しても `Failed` のため child start は無い。
- backend を DirectML へ切り替えたら owner event と App が保持する表示済み stale banner の両方を閉じ、
  owner を disable する。
- pack install 完了は App の stale banner も閉じ、保存 backend を TensorRT にして owner を rearm / hot start する。機能削減や
  app 再起動要求は追加しない。

設定画面の稼働表示は `has_worker_pool` ではなく owner snapshot の `Attached` を projection する。

## 7. exit、cancel、drop

App の通常終了と bypass exit の共通入口から `owner.retire(AppExit)` を一度呼び、Remote bridge や
background request が持つ Arc より先に revision を無効化する。呼出しは待たない。

- `Starting` / delayed retry: current ticket を失効し、task は終了時に stale completion として捨てる。
- `Attached`: routing から即時に外し、pool を background reaper へ移す。
- inference 中: その request の pool lease は terminal cleanup を続けるが、新 request は DirectML。
- stale successful start: background task 自身が pool を reaper へ渡し、App / Remote を再起動しない。
- lifecycle start task panic / reaper 作成失敗: `Starting` を永久に残さず typed deterministic `Internal`
  failure として `Failed` terminal、または exit 中なら `Disabled(AppRetired)` のままにし、Condvar waiter を
  wake する。OS の start task / child process spawn error は `Start(ProcessSpawn)` として通常の 1 回 retry
  policy を通す。
- pack uninstall: `PackUninstalling` transition が新 route と start を遮断し、retire group の全 activity を
  `TrtPackUninstallPermit` が待つ。permit completion より前に filesystem delete や再 install を始めない。
- `AppRetired`: すべての外部 rearm API と demand に対して absorbing。診断 process は retire 前にだけ
  configure / start / terminal wait を行える。

manual request や個別 AI request の cancel は process worker の状態を disable しない。request は route lease を
drop して終わり、共有 worker は他 consumer のため維持する。ただし owner terminal を待つ diagnostic / Remote
helper は request cancel を terminal snapshot より先に確認して `Cancelled` を返す。

## 8. 実装範囲

主な対象は次である。coherent chunk として owner、producer、routing、UI projection、test、docs を同時に
移行し、旧 guard / counter / direct attach API を残さない。

- `src/ai/runtime.rs`
- 新規 `src/ai/trt_worker_lifecycle.rs` と `src/ai/mod.rs`
- `src/ai/upscale.rs`、必要な `src/ai/denoise.rs` / `src/ai/final_pipeline.rs` test
- `src/app.rs`
- `src/ui_dialogs/trt_worker_notice.rs`
- `src/ui_dialogs/preferences.rs`、`src/ui_dialogs/trt_install.rs`
- `src/remote_ipc/session.rs`、`src/remote_ipc/container.rs`
- `src/video/upscale/job.rs`、`src/books.rs`、`src/materializer.rs` は shared entry / owner 保持の照合と必要な test
- `src/bin/bench_ai.rs`
- [architecture-overview.md](architecture-overview.md)、[tensorrt-worker-design.md](tensorrt-worker-design.md)、
  [next-release-backlog.md](next-release-backlog.md)、manual / verification docs の必要箇所

TRT child protocol の §1.241 typed failure と DirectML runtime owner は維持する。変更は親 process の
worker lifecycle と route に限定する。

## 9. 自動回帰

実 process や GPU を使わない fake spawner / fake pool と pure transition test を主にする。

1. lazy demand の deterministic `RuntimeInit` が `Failed` になった後、通常表示、prefetch、Remote、
   materializer を含む N request を流しても spawn count=1、notice=1、全 request が DirectML。
2. banner Close 後も `Failed`、追加 demand で spawn=0。
3. manual restart は 1 click につき 1 attempt。同じ壊れた pack なら再び 1 failure、新 event。
   pack を直した後の manual restart は `Attached`。
4. backend off / on と pack install 完了は revision を更新する。同版 pack install も rearm し、
   旧 revision の成功 / 失敗を拒否する。
5. `Starting` 中および 2 秒 backoff 中の backend off / uninstall / app exit は cancel され、遅い成功を
   attach せず、notice も出さない。
6. transient start failure は exact episode で 1 回だけ再試行。deterministic failure は 0 回。
7. 旧 pool A の遅延 death は新 pool B を外さない。同じ pool の並行 death は transition / retry / event が
   1 回だけ。
8. attach → 最初の inference death を繰り返すと #1 / #2 / #3 は各 1 回自動再起動し、#4 で terminal。
   matching pool の inference success 後だけ death budget が reset される。
9. request route は全 tile で同じ。途中 attach では DirectML 維持、worker death は失敗 tile から
   DirectML へ一方向に fallback。
10. SubjectMatte / MI-GAN / General v3 request は `Eligible` でも start しない。他 5 model は同じ gate。
11. Remote digest generation が pool replace で変わり、read だけでは start しない。
12. Remote acquire barrier は `Starting` のみを新 blocker とし、completion / failure / disable で解除。
13. OS の start task / child spawn failure は typed transient として最大 1 retry、task panic / reaper 作成失敗は
    typed deterministic internal failureとして retry=0。どちらも `Starting` を残さず waiter を有限時間で wake する。
14. pure transition test と API visibility で、`route_or_begin` を `Failed` に何度適用しても revision / state /
    spawn count が変わらず、demand caller が rearm cause を渡す API を持たないことを検査する。
15. static source test で App / UI / Remote / materializer に direct `TrtWorkerPool::start`、direct attach、
    `trt_restart_in_flight`、retry counter が残らず、全 producer が owner APIへ合流することを検査する。
16. pack uninstall の barrier が queued start と active request route の両方で閉じたままになり、route drop
    後も reaper が child shutdown を終えるまで開かない。削除中の install / backend rearm は start を作らず、
    completion 後の次 demand だけが起動する。
17. backend off / pack install は owner event と App 側の表示済み banner を消し、旧 banner revision の
    manual restart を拒否する。`AppRetired` 後は backend / install / manual / uninstall / demand の全入口が
    state と spawn count を変えない。

実装中は narrow な lifecycle / upscale / notice / Remote tests、`cargo check -p mimageviewer --bin
mimageviewer-core`、`cargo fmt --check` を先に行う。共有 runtime、全 AI producer、Remote、App exit に及ぶため、
completion review 後に `scripts/test-full.ps1`、static gate、`scripts/build-release.ps1 -PreserveRuntime` を行う。
既存 §1.241 実機証拠は再実行しない。

## 10. 実機再確認の限定範囲

自動 gate と独立 completion review 後、ClaudeCode release verification へ次だけを追加依頼する。
Codex は GUI を起動しない。

1. disposable portable profile と壊した pack で、複数画像、prefetch、可能なら Remote AI を流す。
   child start / `RuntimeInit` failure は全体で 1 回、各画像は DirectML で完了する。
2. banner を Close してさらに複数画像を流しても start は増えない。
3. 「ワーカーを再起動」を 1 回押すと start が 1 回だけ増え、壊れたままなら再び terminal になる。
4. pack を修復または正規 install し、install hot activation または manual restart の 1 回で attach する。
5. 正常 pack で複数 tile / 複数画像を TensorRT inference し、DirectML fallback、manual restart、Remote を
   含む既存機能が退行していない。

fault injection は隔離 profile の TensorRT pack だけへ行い、通常 `%APPDATA%`、通常 data、署名済み配布物を
変更しない。ORT self-repair、DB locked、Store 再申請、版番号 / changelog / 公開作業はこの確認に混ぜない。

## 11. 対象外

- §1.241 の ort backport、VC Runtime 同梱、launcher self-repair、PE gate の再設計
- TensorRT / CUDA / ORT の版更新、engine 再生成、性能 tuning
- Remote Phase 5、rating sort、collection の追加機能
- UI 機能の削除、TensorRT の自動無効化、壊れた pack の暗黙削除
- user profile や通常 TensorRT pack を使う GUI test

## 12. review checkpoint

- 2026-09-15: §1.241 GPU 実測と current source を照合。6 回の lazy start、分散 state、producer 差、
  old-pool ABA、attach 時 budget reset、stale async completion を同じ lifecycle owner の問題として確定。
- 2026-09-15: 実装担当が本設計を作成。独立 Sol / xhigh review で、worker owner 配置、全 producer、
  revision / pool identity gate、request route、Remote、explicit rearm、exit / reaper の境界に合意。
  初回 finding の death retry off-by-one、OS spawn と task panic の型分離、consumer から rearm authority を
  隠す API 境界を修正し、**製品実装開始可**となった。
- 2026-09-15: 実装中の独立 review で、従来の非同期 reaper と pack directory 削除が競合する境界、
  App 側に投影済みの stale banner、`AppRetired` 後の遅い rearm を検出した。revision ごとの retire group と
  uninstall permit、削除中の rearm 保留、UI projection の明示 clear、absorbing exit state を設計と製品へ
  反映した。
- 2026-09-15: completion review 初回で、runtime route と配布 pack の必須 engine 一覧の二重定義、
  install と uninstall が同時に開いた場合の Preferences projection、実装に存在しない executor channel
  failure 記述、DirectML runtime の Dormant 中にも owner が先に Attached になれる表示境界を検出。
  `TRT_WORKER_MODEL_KINDS` を配布 build と demand gate の正本にし、uninstall 受理後だけ UI snapshot を
  更新し、Preferences は runtime 有無と独立した owner snapshot を使うよう修正した。lifecycle owner の
  API visibility と legacy pool / guard / counter bypass を含む affected-path 再reviewは
  **blocking / should-fix なし**で完了した。
- 2026-09-15: focused は lifecycle **20 / 20**、canonical pack model list **1 / 1**、
  install / uninstall projection **1 / 1**、Dormant runtime と Attached owner の Preferences 投影
  **1 / 1**、`mimageviewer-core` check がすべてexit 0。`colorize_` 同一process groupも
  **29 / 29**である。
- 2026-09-15: 初回 full と serial full は既存の
  `colorize_holdover_survives_explicit_incomplete_final_invalidation`だけが失敗した。focused 単独との差を
  追跡し、test 内の1x1非同期workerがcache削除後に結果を再挿入できるschedule依存を確定した。製品挙動は
  変更せず、exact-key resultをopen receiverでpendingに保持するtest fixtureへ直し、独立review合意後に
  同一sourceでfullを再実行した。final fullは本体 **8560 passed / 0 failed / 45 ignored**、snapshot
  **52 / 52**、vendor egui / egui-wgpu / eframe **25 / 9 / 15**、exit 0。ログは
  `target/section243-final-20260915/test-full-final.{stdout.log,stderr.log,exit.txt}`、SHA-256は順に
  `9FCD06D03EE1EA1E3C8C3144B4B1D0E5C8415786365EEB549308883DD49ED61A`、
  `BBE8FBD65B2E1939AD2B638690329E7F13BDAFC7F20E638C87E774114C75DB1A`、
  `13BF7B3039C63BF5A50491FA3CFD8EB4E699D1BA1436315AEF9CBE5711530354`。
- 2026-09-15: static gateはfmt / glyph / viewer-context audit / diff-checkをfail-fastで実行しexit 0。
  `target/section243-final-20260915/static.{stdout.log,stderr.log,exit.txt}`のSHA-256は順に
  `3E016394B8A58CA0495DCABC1600314F92395A983F58085E313A6B11CE0AE7A4`、
  `62B735DC5D00E487A4AAC7A1946F62B9FB87999621C08BE5FD0EBD8E86C74BAF`、
  `13BF7B3039C63BF5A50491FA3CFD8EB4E699D1BA1436315AEF9CBE5711530354`。
- 2026-09-15: `scripts/build-release.ps1 -PreserveRuntime`はPowerShell 7でexit 0。core / Remote /
  launcher、release runtime **4 / PE 3**、embedded **4 / PE 11**を完了した。最初のWindows
  PowerShell 5.1 invocationは`Get-FileHash`不在でVCRT gate前にexit 1となったため、製品build失敗ではなく
  実行shellの環境失敗として初回ログを保持した。final log
  `target/section243-final-20260915/build-release-final.{stdout.log,stderr.log,exit.txt}`のSHA-256は順に
  `5C186988F62A3B65CEBF3AAF1841DEE79E790C67C9B4D7A72710F26A51200CBF`、
  `DD0D596E3AAC40CDE0785A9D6564CC7C871A7C52D6343AABF079056E799FA24E`、
  `13BF7B3039C63BF5A50491FA3CFD8EB4E699D1BA1436315AEF9CBE5711530354`。
  launcher / core / RemoteのSHA-256は
  `B79C33D364518556F7041AC59CF18A44A75414320EE564340A773F3D1F0270E2`、
  `6FA2166005E121695AD822289E1F18129C6130BAAD478B59600FB3015A8D0E2F`、
  `1B313FF1D89347EAE2F073AFA3C1AB9EC07331825568F96F820A226C3863237A`。
  exact artifact ledgerは`target/section243-final-20260915/BUILD-MANIFEST.sha256`、owned source ledgerは
  `target/section243-final-20260915/OWNED-MANIFEST.sha256`に固定する。
- 残るrelease handoffは、隔離packだけを用いた壊れたpackでの複数画像、通知close、manual restart、
  修復済み正常pack attachの限定再検証である。§1.241で完了したunsigned三形態、Defender有効host、
  DirectML / subject / TensorRT / Remoteの成功証跡は再利用し、GUIはこの実装taskでは起動していない。

## 13. ClaudeCode限定実機結果の受領 (2026-09-15)

ClaudeCodeが `ffeaacfa9` から作成した隔離ポータブル版を操作し、§10の5項目をPASSと報告した。
証跡は [RESULTS.md](../target/section243-verification-20260915/RESULTS.md) と同directoryのrun-A/B/Cログ。
壊れたpackで6枚、通知close後の新規画像を含め14処理でも起動・失敗・子プロセス各1回。
手動再起動ではrevision=2の試行が1回だけ、正常packへ復旧後の手動再起動でrevision=3が25msでattachし、
表示中・先読みの8枚がTensorRTの3モデルで完了した。通常packは読取コピーのみ、隔離packは復元済みと報告。
任意項目のRemote AIは今回未実施で、§1.241の正常Remote結果と自動回帰を再利用する。
これにより§1.243の限定実機完了条件を満たす。最終署名済み配布物の起動検証は公開工程に残る。

別観測として、通知初期位置で再起動ボタンが反応せず、画像のない場所へ通知を移すと反応したケースがある。
手動再起動のPASSは移動後のボタン操作による。別アプリの最前面windowが重なる環境で、fullscreen終了にも
WM_CLOSEを使用したため、原因・通常操作での再現性は未確定。WM_CLOSE原因説は報告者が取り下げ済み。
一覧クリックは反応し、無反応時はManualRestartログが出なかったという観測を保持する。
通知のinput ownership／layer順を既存§1.232等と照合する別調査対象とし、本項目のworker lifecycle成功から
通知UIも問題なしとは結論しない。追加実機操作は具体的な検証枠の了承後に実施する。
