# 動画シークストリップの表示寿命と worker 所有権

最終更新: 2026-09-11  
対象: 次版 backlog §1.211、および 2026-09-11 の動画メモリ急増調査

この文書は、動画 HUD の一時非表示によってシークストリップの thumbnail worker と
decoder が短時間に再生成される問題について、確定した根因と構造修正の受入条件を固定する。
実装前の設計記録であり、この時点では製品コードを変更していない。

## 1. 観測した失敗と守るべき不変条件

シークストリップを表示する設定のまま動画 HUD が自動的に隠れると、native presenter は
`CloseSeekStrip { cause: HudHidden }` を毎 render で生成する。App はこれを通常の close として
処理し、thumbnail worker を cancel/drop して native player の strip payload を消す。一方、
保存された `SeekStripView::Showing` は `HudHidden` では消えないため、次の App update の
`sync_native_video_seek_strip` が同じ動画の strip session と worker を作り直す。

この経路は次の不変条件を破っている。

- 同じ viewer context、同じ source、同じ表示設定の間は、一時的に HUD が隠れても strip
  session と decoder の所有者は変わらない。
- 表示の一時停止と、利用者による close、source 変更、fullscreen 終了などの session 終了を
  同じ close state で表さない。
- native renderer が観測した表示状態は、中間 logical pass ではなく実際に present された最終
  pass から App へ通知する。
- worker、decoder、cell cache、wave worker は viewer context に属し、別 context の同じ
  `fs_idx` や同じ path へ流用しない。

根拠となる現在の経路は次のとおり。

- `src/app.rs`: fullscreen 動画の各 update で `sync_native_video_seek_strip` を呼ぶ。
- `src/video/native_presenter/render_core.rs`: strip payload があり、bottom HUD が隠れていると
  `CloseSeekStrip(HudHidden)` を生成する。
- `src/video/mod.rs`: native overlay command を source epoch 付きの通常 output busへ送る。
- `src/app/native_video.rs`: close 時に thumbnail worker を cancel/drop する。wave worker だけは
  同一動画向け holdover へ移す場合がある。
- `src/video/seek_strip_thumbs.rs`: 一つの安定した worker 内では decoder を lazy-open して保持する。

したがって多数の decoder open はセルごとの通常動作ではなく、session/worker が再生成された
ことを表す。

## 2. 確定事項と未確定事項

### 確定したこと

- 凍結ログには `HudHidden` close が 24,053 回、decoder ready が 1,054 回あり、decoder の
  thread ID は 988 個だった。
- AV1 1920x1080 では 17.7 秒に 593 decoder、WMV3 640x480 では 3.52 秒に 141 decoder が
  作られた burst がある。
- 現コードの `HudHidden -> terminal close -> persisted Showing -> ensure/open` がこの再生成を
  起こす。
- thumbnail worker 内の decoder は通常一 session に一つであり、安定した session 内で
  各セルごとに open する設計ではない。

詳細な観測値は `target/video-memory-20260911-1112/analysis-report.md` と同じ証拠ディレクトリの
JSON/logを参照する。

### 未確定のこと

- PID の OS 累積 `PeakWorkingSet64` は 21.33 GiBだったが、その peak 時刻は記録されていない。
  decoder churn がこの全量を直接保持したとは証明できていない。
- WDDM `process_dedicated_bytes` は physical VRAM 使用量ではない。DirectComposition/DWMを含む
  PID attributionとして物理容量を超えることがあるため、今回の根因や計測バグの直接証拠には
  使わない。
- §1.211 の「whole strip が中央約60%に8〜9セルだけ出て、HUD hide/show後に直る」症状は、
  session再生成で`visible_count`が初期値9へ戻り、presenterの実幅feedback
  `RequestSeekStripWindow`が定着しない説明と整合する。ただし当該frameのcell-count eventを
  記録していないため、同じ寿命不整合が直接原因だとはまだ証明できていない。
- 既存のpure layout計算は全幅を埋める。追加証拠なしに別のlayout math bugと断定しない。

## 3. 正本となる typed state

`HudHidden`を`SeekStripCloseCause`から外し、sessionの有無とpresent状態を別の型で表す。
概念上の状態は次の形とする。実装時の型名は既存moduleの命名へ合わせてよい。

```rust
enum SeekStripRuntime {
    Closed,
    Open(SeekStripSession),
}

enum SeekStripPresentationActivity {
    AwaitingFirstPresent,
    Visible,
    Suspended,
}

enum SeekStripPresentedInventory {
    Absent,
    Hidden { session_id: SeekStripSessionId },
    Visible { session_id: SeekStripSessionId },
}
```

`Open`のsessionは最低限、次のidentityを持つ。

- bundle-localで単調増加する`session_id`
- `items_generation`
- `fs_idx`
- native source epoch
- video path
- mutableなmode/span、axis/center、layout revision

resource sessionの一致判定はpathだけにしない。同じpathの明示close/reopen、別viewer contextの
同じ`fs_idx`、source replacementを`session_id + items_generation + fs_idx + source epoch`で区別する。
mode/span/height/resizeやF12 placement switchはresource sessionのterminalではない。これらが変わっても
thumbnail/wave workerとdecoder identityを保持する。

`session_id`は`NativeOverlaySeekStrip`からrendererの最終inventory、native output event、Appの
runtimeまで欠落なく運ぶ。Appは現在Open中のexact idとowner identityが一致するeventだけを
受理する。古いHidden/Visible eventが同じsource epoch内で再openされた後継sessionへ作用しては
ならない。

同じstampはHidden/Visibleだけに限定しない。strip payloadから生成され、現在のsessionを参照または
変更するnative overlay commandを次のように分類する。

- session生成前の操作: `OpenSeekStrip`。active sessionがないためsession idは持たないが、producerの
  window/placement generationは持つ。
- session-bound: `CloseSeekStrip`、`MoveSeekStrip`、`CommitSeekStrip`、
  `RequestSeekStripWindow`、`StepSeekStripRange`、`ToggleSeekStripLock`。すべてexact session idと
  producerのwindow/placement generationを持つ。
- menu configuration: `SetSeekStripView`と`SetSeekStripHeight`はstripがClosedでも表示され得る。
  `expected_session_id: Option<_>`とproducer generationを持たせ、`Some(id)`はexact Open session、
  `None`は現在もClosedである場合だけ受理する。古いrendererの`None`をOpen中sessionへ適用しない。
- strip非依存: 通常のseek、hover thumbnail、動画navigationなど。strip session stampを足さない。

特にlatest/coalesce対象の`RequestSeekStripWindow`、およびlayout上の座標を運ぶ`MoveSeekStrip`と
`CommitSeekStrip`はlayout revisionも持つ。mode/span/height変更ではrevisionを進めて旧layout由来の
feedback/gestureを拒否するが、同じworker handleを新しいmutable configへ移して保持する。resizeは
同sessionのlayout feedback更新として扱い、worker/decoderを再生成しない。`StepSeekStripRange`等の
連続した利用者commandはlosslessな順序を維持し、一つ目の適用だけで同じ入力batchの後続stepを
誤って捨てるrevision運用にはしない。

## 4. rendererからAppへのpresent済みedge

native rendererは、logical outputに`SeekStripPresentedInventory`相当を含める。

1. 各logical passは、そのpassでstrip payloadが無い場合を`Absent`、payloadがありbottom HUDが
   隠れている場合を`Hidden{id}`、実際に描画した場合を`Visible{id}`として記録する。
2. 一つのnative event batchで複数logical passを実行した場合、batch inventoryは最終passの値で
   上書きする。unionや全passのedge列にはしない。
3. `surface_texture.present()`だけでなく、その後の`set_visual_attached(true)?`など当該batchの
   fallible処理がすべて成功し、`present_logical_batch`が`Ok`を返す直前に、前回committed
   inventoryとの差をrender core内のlatest pending snapshotとして記録する。
4. pending snapshotを`NativeOverlayInputOutcome.commands`だけへ入れない。通常frame、resize、grade、
   zoom、show refresh、placement candidate primeなど、成功outcomeのcommandsを呼び出し側が使わない
   既存経路がある。そこでnative output loopは各iterationの末尾にcurrent presenterのpending
   snapshotを読み、latest-value eventとして必ず送る。
5. 同じ値のframeではpendingを繰り返さない。present/acquire/render/visual attach失敗時はcommitted
   stateもpendingも進めない。output loopはpendingをpeek/cloneし、専用latest publisherが受理した
   後にだけackしてclearする。現在の`send() -> ()`へ「成功したはず」と委ねない。publisherは
   `Result`等でtyped accepted/faultを返し、Mutex poisonなど受理不能ならpendingを保持したまま
   native outputをfatal terminalへ移す。
6. placement candidateがprime中に作ったpendingは、candidateがcommitされcurrent presenterへ
   replaceされた場合だけ送る。candidateがabortした場合はcandidateと一緒に破棄する。
7. candidateの最初のvisible passはpresentation inventoryだけでなく、実寸の
   `RequestSeekStripWindow`を生成してrenderer内のrequest dedup正本も進める。現行のplacement primeは
   `overlay_outcome.commands`を利用しないため、このrequestをその場で捨てるとcommit後にも再送されず、
   Appの初期`visible_count=9`が残る。candidate outcomeの全semantic commandを再生するのではなく、最終
   `Visible` inventoryとsession/layout stampが一致するlatest passive `RequestSeekStripWindow`だけを
   candidateと一緒に保留する。placement commit後にcandidate generation付きで一度publishし、abort時は
   破棄する。dedupは、このcommit/abort所有者から確実に配送されるrequestだけ進んだ状態として扱う。
8. `SetSeekStrip(None)`、overlay source session reset、`SwitchSource`、render core replacementでは
   renderer側のcommitted baselineとpendingを`Absent`へclearする。

このeventは利用者操作ではなく、現在のpresent済み状態である。native output busではlosslessな
操作列として蓄積せず、sequenceを持つlatest-value slotに載せる。eventはsession idと
window/placement generationを持つ。App側の既存source epoch gate、playerのcurrent generation、
exact session identityをすべて照合する。

output threadはold/new presenterを単一threadで逐次処理するため、raw HWNDやviewer context idを
eventへ足さない。viewer context ownershipは、そのplayerのoutput busを現在mountしてdrainする
bundleによって決まる。

`Absent`はrendererのbaseline/resetだけに使い、idを持たず、App runtimeをclose/suspend/resumeする
eventにはしない。Appのterminalは利用者command、source/context lifecycle、availabilityから決める。
遅延した`Absent`に意味を持たせない。

通常のoverlay commandsはpresent成功outcomeから先にoutput busへ送られ、presentation pending snapshotは
output-loop iteration末尾にpublishされる。この順序では、最初の成功visible passが生成した
`RequestSeekStripWindow`等が、対応するVisible edgeより先にAppへ届く。Appのsession-bound command
gateは`AwaitingFirstPresent`を失敗扱いにせず、exact session id、placement generation、必要なlayout
revisionが一致するcommandを受理する。`Suspended`だけが新しいlayout requestやgestureを拒否する。

新event handlerから`mark_native_video_hud_activity`を呼ばない。現在のClose handlerと同じ副作用を
残すと、表示観測がHUD visibilityを変え、再び自己励起する。

このpresentation eventはParkedLive maintenanceだけでなく、sidecar restore modal中にも許可する。
semantic inputではなくpresenter lifecycle resultなので、modal中に捨てるとrendererだけがHiddenへ
commitしてAppがActiveのままになる。どちらの経路でもactivationやHUD activityを起こさず、exact
ownerへ適用する。

sidecar restore modal中は、同じpresentで先に届くexact `RequestSeekStripWindow`もpassive feedbackとして
許可する。これを捨てるとrendererの`last_window_request` dedupによってmodal解除後に再送されず、
初期9セルが残り得る。対象sessionがSuspendedなら拒否する。Move/Commit等のsemantic inputは従来の
modal blockを維持し、modal中のpassive例外へ広げない。

## 5. transientとterminalの遷移

| 契機 | 分類 | session/worker | 保存された表示状態 |
| --- | --- | --- | --- |
| 初回成功present中のexact layout feedback | Awaitingで受理 | 同じsessionへ一度適用 | 変更しない |
| present済みHUD Hidden | transient suspend | 同じsessionを保持 | 変更しない |
| 同じidのpresent済みHUD Visible | transient resume | 同じsessionを再開 | 変更しない |
| 利用者Toggle/下drag/Escape | session/thumbnail terminal | thumbnailをcancel/drop。waveは同動画holdover可 | Closedへ保存 |
| VideoChanged/source epoch変更 | resource terminal | sessionとholdoverをcancel/drop | 現仕様の選択保持を維持 |
| viewer fullscreen exit/context retire | resource terminal | sessionとholdoverをcancel/drop | 現仕様を維持 |
| native placement switch/presenter再作成 | nonterminal transfer | 同じsession/workerを新coreへprime | 変更しない |
| TileModeOpened | session/thumbnail terminal | thumbnailをcancel/drop。waveは同動画holdover可 | 現仕様を維持 |
| audio-only/no source | resource terminal | sessionとholdoverをcancel/drop | 現仕様を維持 |
| material Unavailable/error | session/thumbnail terminal | thumbnailをcancel/drop。waveはcauseに従いholdover | 現仕様のnotice/closeを維持 |
| mode/span/height変更 | same resource session | worker/decoderを保持しconfig/revisionだけ更新 | 新しい設定を保持 |
| resize/layout count変更 | same resource session | request/axisだけ更新 | 変更しない |
| 通常のbundle mount/deposit | ownership transfer | handle identityを保持 | 変更しない |

Hiddenはterminal closeの一種ではないため、`SeekStripCloseCause::HudHidden`を残して別分岐する形には
しない。

現行のwave holdover契約を維持する。`keeps_viewing_the_same_video()`に該当する利用者close、tile、
Unavailable等ではthumbnail sessionを閉じてもwave workerをbackground pauseしてbundle-owned
holdoverへ移し、同動画の再openやmode切替で再利用できる。VideoChanged、source replacement、実際の
viewer fullscreen exit、context retireではholdoverも終了する。mode切替だけを理由にwave/thumbnail
workerやdecoderを再生成しない。

## 6. Suspended中の作業

`sync_native_video_seek_strip`はSuspended中にも、次のterminal条件だけを確認する。

- exact source/session identityが現在も有効か
- user-visible settingがShowingのままか
- tile/audio-only/no-source/Unavailableへ移ったか

mode/span/height変更はterminal判定ではなく、Suspended中も同じresource handlesへ反映するconfig更新で
ある。layout revisionを進め、Visible再開時に新configから一度だけlayout/requestを作り直す。

ここでいうconfig更新はproducerを区別する。root/settings/keymap等のApp正規入口で確定した設定変更は、
Suspended中も現在sessionのmutable configへ適用して保存し、layout revisionを進める。既存のthumbnail/
wave workerとdecoderはcancel/dropしない。一方、Hiddenになる直前の旧native presentationが生成し、
Hidden edgeより遅れて届いた`SetSeekStripView` / `SetSeekStripHeight`等のconfig commandは拒否する。
「Suspended中のconfigを拒否する」はこの遅延native commandだけを指し、App正本の設定変更を無視する意味
ではない。

正規mode変更に必要なworkerがまだ存在しない場合（例: thumbnailからwaveformへ変えた時の
`wave_worker=None`）は、sessionのmode/revisionだけを更新し、Hidden中にはworkerをspawnせずrequestも
出さない。exact Visible resume後の一度syncで必要なworkerを作り、最初のrequestを発行する。すでに存在
する他modeのworker/decoderは同じsessionに保持し、この延期を理由に作り直さない。

terminal条件が無ければ、次を行わない。

- playback follow/recenter
- thumbnail/window/wave foreground requestの新規発行
- thumbnail/wave snapshotのcloneとoverlay payload更新
- visible cell stall計測・報告
- strip固有の定期repaint

command受理はactivityごとに次を守る。

- `AwaitingFirstPresent`: 同じsuccessful visible passから返ったexact owner stampの通常commandを受理
  する。RequestWindow、Move/Commit、StepRange、ToggleLock、user Close、
  `expected_session_id=Some`のSetView/SetHeightを、Visible edgeより先という理由だけで失わない。
- `Visible`: exact stampを持つ通常のstrip feedback/gesture/config commandを受理する。
- `Suspended`: 新しいRequestWindow/Move/Commit/StepRange等を拒否する。利用者closeやsource/contextの
  terminalは別のlifecycle経路で引き続き処理する。

Hidden直前に受理済みだったthumbnail requestまたはwave foreground requestは完走してよい。新しい
requestを出さず、結果は同じworkerのSharedStateに留める。wave workerのfull-length background
analysisだけは既存`set_background_paused(true)`でpauseする。

したがってHidden適用後でも、受理済みrequestがまだdecoderをlazy-openしていなければ、そのworkerが
decoderを一度openし、request範囲内のcellをpopulateする場合がある。これを新規Hidden作業と誤分類
しない。禁止するのは新しいdesired window/request id/session/workerの生成と、同一workerでのdecoder
再生成である。

exact idのVisible edgeを受けたら、wave backgroundを現在modeに応じてresumeし、hidden時間を
visible stall時間へ算入しないようpending timingをresetする。その後、現在のplayheadとpresenterが
報告したlayoutで一度syncする。Focus、timer、retry、余分なrepaintでvisibilityを推定しない。

## 7. ViewerContextBundle所有

現在App-globalであるstrip runtimeとwave holdoverは、context-scoped resourceとして
`ViewerContextBundle`へ移す。bundle-localのnext session idも同じownerへ置く。

- ordinary AtRest mount/depositはruntime、worker handle、cache、holdover、next idをswapし、同一
  identityを保つ。
- LiveMedia fork/transferはnative playerと同じtransactionでstrip ownerをmoveする。playerだけを
  移してstrip workerを元contextへ残さない。
- 新規fork/open siblingは空のruntimeと独立したnext idを持つ。worker/cellをcloneしない。
- `items_generation` replacement、index-space replacement、context retire/dropではそのbundleの
  sessionだけをcancel/dropする。
- mount/depositそのものをterminal扱いしない。F12 viewportは毎frame bundleを交換するため、ここで
  cancelすると一時非表示修正後もworkerが継続できない。
- raw HWNDをAppのsession identityへ追加する必要はない。HWNDはnative eventのhost/placement経路で
  検証し、App resourceの正本はbundle containmentとsource identityに置く。

Suspended sessionをF12 placement switchで新しいrender coreへ渡すとき、strip payloadを失わない。
output loopが保持する既存の`cur_*` stateと同じく、`cur_seek_strip`とmaterial availabilityを保持し、
placement candidateをprimeする前に再適用する。placement commit後にAppから通常requestを再発行して
埋める方式は採らない。Hidden中の「新requestを増やさない」契約を破るためである。

late worker completionは元workerの`Arc<SharedState>`内だけにpublishされる。terminal後にhandleを
dropすればAppから到達不能となり、後継sessionへ結果をコピーしない。cancelと完了が競合しても
successorのSharedStateへ入らないことをArc identityの回帰で固定する。

## 8. メモリ境界

thumbnail cell cacheの128 MiBはstrict hard capとは記述しない。evictionは要求開始前に行われ、
現在のrequest/pinned windowを保持するため、完了直後にはsoft budgetに現在窓ぶんの決定的な超過が
あり得る。whole spanは最大512セルでboundedである。

今回の修正は、Hidden中の保持をOpen時と同じ一session、一worker、一decoder、bounded request window
に留める。cache budgetやdecoder pool自体の再設計は含めない。

既存wave側の境界も維持する。

- coarse full-length analysis: 64 MiB上限
- raster LRU: 8件
- raster width: 8192上限
- Hidden中はbackground analysisをpause

Hidden適用後に新しいsession、worker、desired window、request idを作らないことをsoft budgetの説明と
独立して検証する。同じworkerのdecoder生成は最大一回で、Hidden前に受理済みのin-flight requestが
Hidden後に行う初回openは許容する。cell populateもその受理済みrequest範囲に限定する。

## 9. 実装候補ファイル

実装時に次をまとめて所有し、compile可能な一chunkとして配線する。

- `src/video/seek_strip.rs`
  - typed presentation/session identity、遷移reducer、pure tests
- `src/video/native_presenter/render_core.rs`
  - 最終logical pass inventory、present成功後edge、committed baseline、renderer tests
- `src/video/mod.rs`
  - typed native event、latest-value slot、output-loop pending回収、`cur_seek_strip`のplacement再適用、
    source reset/SwitchSource lifecycle
- `src/app/native_video.rs`
  - exact event handler、Suspended work gate、resume/terminal処理、session identity
- `src/app.rs`
  - App投影fieldと初期化
- `src/app/viewer_context_registry.rs`
  - bundle field、mount/deposit、LiveMedia transfer、fork/retire/drop
- `src/app/snapshot_ops.rs`
  - snapshot/restore/remap時のtyped runtime処理が必要な場合
- `src/app/tests.rs`および各moduleの局所tests
- `docs/video-architecture.md`
- `docs/video-seek-strip-plan.md`
- `docs/async-architecture.md`
- `docs/detached-rework-plan.md` §11

`app.rs`、`native_video.rs`、`viewer_context_registry.rs`等は別chunk §1.209 と共有する。所有権返却前に
一部だけを編集せず、返却後にrenderer、event、App、bundleを一括してcompile可能にする。

## 10. 必須回帰

### 根因と表示edge

- HUD Visible→Hiddenを多数frame描画してもHidden edgeはpresent成功時の一回だけ。
- 一batch内の中間pass Hidden、最終pass VisibleではVisibleだけをcommitする。逆順も同様。
- surface acquire/render/present後処理の失敗ではcommitted inventoryとApp eventを進めない。
- 通常frame/resize/grade/zoom/show refreshでoutcome commandsを無視しても、iteration末尾のpending回収で
  edgeを一度送る。
- latest publisherが受理した後だけpendingをackする。受理不能はsuccess扱いせずoutput fatalとなる。
- placement candidate primeのedgeはcommit後だけ送り、abort時は送らない。
- source reset/SwitchSource/core replacement後に古いinventoryを再利用しない。
- placement candidateの最初のvisible passが生成したlatest exact `RequestSeekStripWindow`は、candidate
  commit後に新generationで一度だけpublishする。abortでは破棄し、renderer dedupだけを進めてrequestを
  失う状態を作らない。candidateのsemantic commandはこのpassive配送へ混ぜない。
- Hidden event handlerがHUD activityを生成しない。
- presentation eventをParkedLive maintenance allowlistへ含め、activationや利用者入力として扱わない。
- sidecar restore modal中もpresentation eventをexact ownerへ適用し、modal解除後のedge再送へ依存しない。
- sidecar restore modal中もAwaiting/Visibleのexact RequestWindowだけはpassive feedbackとして受理し、
  semantic inputはblockする。SuspendedのRequestWindowは拒否する。
- `Absent`はApp eventを作らず、runtimeを変更しない。

### session identity

- 同source epochのclose→reopen後、旧session idのHidden/Visibleと全session-bound strip commandを拒否する。
- 同じpath、同じ`fs_idx`でも別viewer contextのeventを拒否する。
- `items_generation`またはsource epochが変われば旧sessionをterminalにする。
- window/placement generationが変われば旧eventを拒否するが、F12 placement switchでは同じsessionと
  workerを保持する。
- Close/Move/Commit/RequestWindow/StepRange/ToggleLockのsession stamp欠落をcompile/pure分類で検出する。
- Openはgenerationだけ、SetView/SetHeightは`expected_session_id`のSome/None preconditionを持ち、
  Closed由来の遅延commandをOpen sessionへ適用しない。
- mode/span/height変更でlayout revisionを進め、旧RequestWindow/Move/Commitを拒否しつつ、同じ
  thumbnail/wave workerとdecoder identityを保持する。
- 同じsuccessful first-present batchの`RequestWindow -> Visible`順でRequestWindowを一度適用し、
  `AwaitingFirstPresent`を理由に拒否しない。
- Awaitingのclassifier/reducerは、同じsuccessful visible passから返るsession-bound commandと
  `expected_session_id=Some`のconfiguration commandを網羅する。代表RequestWindowの順序testに加え、
  許可集合の分類testでMove/Commit/StepRange/ToggleLock/user Close/SetView/SetHeightの欠落を防ぐ。
- Suspendedでは新規feedback/gesture/config commandを拒否する。exact user Closeはterminalとして扱う。
  stamp mismatchはactivityにかかわらずすべて拒否する。
- Suspended中のApp正規設定更新はmutable config/layout revisionへ適用・保存し、既存worker/decoder
  identityを保つ。同じ変更を旧native presentation由来の遅延config commandとして受けた場合だけ拒否する。
  mode変更で必要なworkerが未生成ならHidden中はspawn/requestせず、Visible後の一度syncで生成する。

### Suspended work

- Hidden適用後に新しいdesired window、request id、session、workerを作らない。
- Hidden前に受理済みのrequestは完走可で、その範囲のcell populateと同worker最初のdecoder openを
  許容する。同workerでdecoderを二つ以上生成しない。
- Hidden直前のin-flight completionは同じsessionへ届くが、後継sessionへは届かない。
- wave backgroundはHiddenでpause、Visibleで現在modeに従いresumeする。
- Hidden中にstrip由来の80ms repaint、follow tick、stall reportを発生させない。
- Visibleで同worker identityを維持し、現在playhead/layoutのrequestを一度発行する。

### layout feedback / §1.211

- whole spanの初期9セルに対し、presenterが報告した実visible countを同sessionへ適用する。
- HUD hide/showを挟んでもvisible countとwhole axisを初期9へ戻さない。
- resize/height変更は同sessionの軸を再構築し、worker/decoderを再生成しない。
- mode/span/height変更前のlayout revisionを持つ遅延RequestWindow/Move/Commitを拒否する。
- pure layoutは既存どおり利用可能幅を埋める。

### context lifecycle

- ordinary detached mount→deposit→remountでsession/worker/idを保持する。
- 同じidx/pathを持つsibling contextは別runtimeで、結果とeventを相互消費しない。
- LiveMedia transferでplayerとstrip runtimeが同じcontextへ移る。
- Suspended中のF12 placement switchは`cur_seek_strip`をcandidateへprimeし、session/request idを変えない。
- F12 commit後は新generationのeventを受理し、旧placement generationのeventだけを拒否する。
- fork/open siblingはClosedで始まる。
- context retire/drop、fullscreen exit、source replacementはowner workerだけをcancelする。

### 既存機能

- user Toggle、DownwardDrag、Escapeの保存とclose挙動を維持する。
- TileMode、audio-only、Unavailable、動画変更、fullscreen exitを維持する。
- same-video close/再openとmode切替でwave holdoverを再利用し、VideoChanged/FullscreenExit/context retireで
  holdoverを終了する。
- thumbnail/waveform、whole/window span、follow、drag、seek、resizeのenabled経路を維持する。

## 11. 非対話検証と実機確認

実装後はpure reducer、renderer final-pass/present、App handler、bundle lifecycleのfocused testsから始める。
共有contextとnative output busへ達するため、最終sourceでnormal core check、test-script check、fmt、
UI glyph、viewer context audit、full gateを一度実施する。

Windows native presenterの実表示、HUD fade、decoderの実寿命はunit testだけでは完了扱いにしない。
使い捨てportable環境で次を確認する実機suiteを別途計画し、起動・foreground・入力範囲について
実行前に利用者の明示承認を得る。

- whole thumbnail stripを開き、HUD auto-hide中にworker/decoder countが増えない。
- HUD再表示で同session idのまま全幅cell countを維持する。
- source switch、fullscreen exit、F12 transferで旧ownerだけが終了する。
- waveform backgroundがHidden中にpauseし、Visibleで再開する。

21.33 GiB peakの解消をこのsuiteだけで断定しない。必要なら同じtimestamp上へWorking Set、Private
Bytes、strip session/worker/decoder identityとopen/close countを追加し、以前のpeakとの比較を別の
計測chunkとして行う。
