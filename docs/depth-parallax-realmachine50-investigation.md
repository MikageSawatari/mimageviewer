# §1.767 第2段 — サブPC依頼50の調査と修正設計（2026-11-14）

基点は `feat-767-stage2` / `d96e47c0b`、製品コードは `178a3ece2`。
今回は調査・設計・失敗回帰テストだけで、製品コード・ログ・vendorは変更しない。
第2段は単独リリースとし、時間より所有構造の正しさを優先する利用者方針に従う。
本稿の設計は実装前の提案であり、設計担当の検収と独立設計レビュー後に実装範囲を確定する。

## 1. 証拠と判定の範囲

- 実機観測者はすべてサブPC担当。`C:/miv-subpc/results-50/notes.md` と
  `C:/miv-subpc/steps-767-stage2.md` の初めから終わりまでを照合した。
- 通常ログは `results-50/part2/logs/mimageviewer.log.prev`（手順10〜15）、
  同 `mimageviewer.log`（手順17〜22）、`part1/logs/mimageviewer.log.prev` / `.log`。
  本担当は製品を起動せず、以下の実機結果を再観測したとは扱わない。
- 「コードで確認」は現ソースの処理、「失敗テスト」は合成入力・headless ownerの結果、
  「推測」は実機で未記録の過程を示す。通常ログにないmouse down/up、watch sample、
  OS foregroundの瞬間値を補って原因を断定しない。

| 件 | コードから特定した故障機構 | 実機との対応の限界 |
| --- | --- | --- |
| A（違った1・2） | 停止した静止画のdeferred callbackは正常なprimary clickを復帰要求へ渡さず、OSのボタンlevel pollingだけに依存する。poll間に完結したclickを失う経路がある | 今回の実clickがpoll間で完結したか、別の理由でwatcherが拒否したかは未記録。Aの実機原因は未確定 |
| B（違った3） | 外部openのmain側leaf採用がgridのhandoffを迂回し、旧独立contextをpauseせずactive sessionを書き換える。旧Depthとbindingが孤児化する | 実機のsession3→4、pause不在、ctx3残留と整合する。旧HWNDが消えた正確なOS時刻は未記録 |
| C（関連するHWND衝突） | キーsubclassだけがrectでHWNDを選び、別viewportに登録済みの旧窓を選ぶと新窓へsubclassが入らない | 同rectの誤同定機構は再現対象。Aの共通原因とは断定しない（A2の発生前に同衝突はなく、交換後もpause前の入力は動いていた） |

## 2. A — 停止窓の復帰要求が届かない

### 2.1 観測とコード

実機では `log.prev:2592`（122.497s）、`:7961`（338.747s）で
`pause_active_context` がctx1 / ctx2のsessionを解放した。
複数ウィンドウでは `log:1414`（70.685s）で窓1をpauseし、`:1755`（90.753s）で窓2をunbindした。
その後、窓1の中をクリック・Dしても反応せず、×では閉じた、というサブPC担当の観測である。
`:1772`（105.622s）の11,343.7ms、`:1780`（131.372s）の21,933.4msはrootのframe gapであり、
watcherの停止やUI threadのdeadlockを示す計測ではない。

コード上の経路は以下。

1. `src/app.rs:65305` `pause_viewer_context_for_handoff` がsnapshotを保存し、
   sessionを解放してruntimeをParkedにする。`reset_detached_pause_foreground_modes`（`:65415`）が
   所有contextのDepthを終了する。これ自体は3Dあり／なしで同じ復帰経路になる。
2. `src/ui_fullscreen.rs:24383` 以降がpassive snapshotのdeferred viewportを登録する。
   callback（`:24520`以降）はfocus、geometry、close、right-drag、page-overviewをrootへ送るが、
   通常のprimary clickの復帰要求を送らない（`DeferredDetachedImageWindowEvent` は `src/app.rs:2185`）。
3. `src/app.rs:3723` `deferred_detached_activation_watch_targets` はsnapshot・再開可能なbinding・
   Parked・生存HWNDから対象を作る。parkの次のroot frameからeligibleになる。
4. `src/app/detached_window_manager.rs:413` のwatch loopがOS sampleからdown/upを推定する。
   周期は固定8msではなく、矩形取得と `WM_NCHITTEST`（最大50ms待ちを含む）の後に8ms sleepする。
   `GetAsyncKeyState` の現在levelで、両sample間に完結した押下は観測できない。
5. 観測できた要求は `src/app.rs:3813` のdrain→既存deferred activation intent→
   `commit_pending_deferred_detached_window_activation`（`:3860`）へ進む。
   rootの更新順（`:101461`付近）に明白なdrain漏れは見つからない。

通常のpassive登録がParkedをActiveへ戻す経路、兄弟closeが残った窓のwatch targetを消す経路は
確認できなかった。`[fs-focus] prev_foreign=true` は「前のHWNDと対象HWNDが違う」という判定
（`src/ui_fullscreen.rs:39903`）で、mIVのHWND登録を失った証拠ではない。

backend側も確認し、deferred登録は `vendor/egui/src/context.rs:5132` でclass / callbackを更新し、
`vendor/eframe/src/native/wgpu_integration.rs:2310` は既存native viewportにも毎出力反映する。
MouseInputは `vendor/egui-winit/src/lib.rs:416` でrepaint対象となり、通常はクリック先childを駆動する。
Immediate→Deferredの切替だけで旧callbackに固定される経路は確認できなかった。
一方、native ×は同wgpu integrationの`:1350`付近のclose dispatchがUI callbackなしでも扱う。
「×が効く」だけではdeferred callbackが動いたとは言えず、今回clickがcallbackまで届いたかも未観測である。
Parkedには通常のD／ページ送りconsumerがなく、activation未確定ならキーも動かない説明とは整合する。

### 2.2 失敗回帰の意味

`src/app/multiwindow_scenario_tests.rs` の既存driverにchild eventsの入力を足す。
新規テストは通常のgrid handlerで独立窓を2つ開き、先の窓が実際にpauseされることを確認し、
後の窓を実close handlerで閉じる。残った窓のbinding・watch target・eligibleを検査したうえで、
そのdeferred callbackへ同じpass内のprimary press/releaseを渡す。watch sampleは前後ともupとする。
controlとして、同じtargetのdown/upを両方sampleした本番reducerが要求を作ることも検査する。
activate / queueを直接呼んで復帰を作らない。

- `realmachine50_parked_depth_window_click_reactivates_after_sibling_close`
- `realmachine50_parked_flat_window_click_reactivates_after_sibling_close`

期待は同じwindow / contextへの復帰、passive snapshotの消費である。
これはcallbackに届いた有効なclickを失うcounterexampleであり、実機のwatcher timingを測る試験ではない。
Depth fixtureは `DepthViewState::new` をpauseで終了する状態検査で、実3D/GPU描画をfreezeする試験ではない。
以前のscenarioがqueue / activateを直接呼んでいたため検査しなかった前半の境界を対象にする。

### 2.3 修正案（BA-7、CのBA-1と接続）

復帰の正本を、所有viewport / HWNDから届いた離散入力にする。OS level pollingを復帰の唯一のproducerにしない。

- clientのclick / tapは既存のdeferred callbackの正規化済みpointer操作から既存event transportへ送り、
  managerの既存 `DetachedActivationIntent` に収束させ、rootの既存commitだけがcontextを切り替える。
  layout replay、drag、bar close、page-overview、modal保護と競合しない一操作一確定を検査する。
  最初のclickは復帰だけに使い、復帰後のページ送りへ再配送しない。
- non-clientのclickは、Cで確定したHWNDの既存subclass／backend input ownerから離散イベントとして受ける。
  clientとのproducerを入力の種類で分け、同じ操作をcallbackとwatcherが二重に確定しない形にする。
  drag / capture lost / HWND終了はその既存gesture ownerで終端する。タイトルバー操作を黙って削除しない。
- focus metadataだけでの復帰は追加しない。現行の「最初のクリックで復帰」を維持する。
  keyboardだけの復帰もこの変更に混ぜない。メディアの既存native入力経路は同じintentへの配送を維持する。
- 新しいApp pendingや時間窓を作らず、既存callback batchとactivation intentの受理／commitを再利用する。
  rootのwakeは既存のevent配送の責務にそろえ、症状用の追加repaintは足さない。

polling周期短縮、eligibleへの例外追加、focusで強制resume、pause後の一括resetは採らない。
閉じて再openする簡素化はページ・zoomなどを失い、detached計画§6のpaused bundle維持に反するため採らない。
実装前レビューではnon-client / touchも含む入力producerの対応表と一意性を確定する。
この修正だけで今回のAが直ったとは扱わず、§6の診断と実機再確認を残す。

### 2.4 A 実装記録（2026-11-14、BA-7）

設計担当の決定と独立設計レビュー（Sol / xhigh、2026-11-14）のP2を反映した。
これは入力の所有・確定点を離散イベントへ集約する構造的な変更であることに双方が合意した。
Cのnative allocation契約を参照し、Cのeframe/backend hookは変更しない。Aの入力正規化通知を
`vendor/egui-winit`へ、非操作のprovenance markerを`vendor/egui`へ追加した。Bは今回の対象外。

| 入力 | producerと確定 |
| --- | --- |
| client click / 単指tap | deferred callbackの`DeferredClientActivationGesture`。順序付きraw primary edgeとeguiのclick判定、既存TouchFrameのraw Touch／昇格Pointer相関で完結した操作だけを既存event queueへ送る。最初の操作は復帰だけに消費しactiveへ再配送しない |
| non-client | Cの確定HWNDに付いたkey subclassのnative input owner。`Mouse / Pointer(id)`が押下からDefSubclassProcのnative操作終了／取消まで所有する。既存WH_GETMESSAGEのPM_REMOVE観測でnative move/resize loopが消費する終端も同ownerへ渡す。昇格NC mouseはraw pointerの第二producerにしない |
| page-overview / right-drag | 既存command ownerを維持。overview面全体・secondary操作は通常clickのproducerにしない。認識済commandは同じmanager intentの中でactivation-onlyより優先する |
| bar close / native close | clientの同gesture ownerがStart時に`WindowClose(close_rect)`を確定し、同じcontrol内での完結だけを既存`Frame.bar_close_requested`へ渡す。outside／drag／取消は終端し復帰へ転用しない。keyboard／accessibilityの既存widget factとbackend native closeは維持する。closeが先に確定し、同操作からactivationを作らない。non-clientの移動・resize・最大化・閉じるはDefの処理を継続する |
| native media | 既存native入力経路から生成時identityを持つ同じDiscrete activation intentへ配送。従来のID通知は配送のackとしてdrainし、破棄・消費済の要求をIDから作り直さない |

**撤去した責務。** managerのwatcher worker、8ms sleep、GetAsyncKeyState level sampling、
rect／foregroundからの候補推定、close hit-test、watch step／target／drain、watcher HWND repairを撤去した。
watcherのclose責務は上表のUI／native closeへ移し、HWND登録はCのbackend正本だけを参照する。
既存のParked HWND livenessと再生成時のpost-show adoptionは維持した。
focus metadataは表示情報の更新だけに使い、focusだけ・keyboardだけの復帰を追加していない。
architecture-overviewのmanager記述を更新した。ui-responsiveness／async-architectureのwatcher記載は
索引・folder等の別workerの記述で、削除対象のactivation workerを定義していないことを確認した。
detached計画の過去の実装記録は履歴として残し、§11に今回の撤去を記録する。

**一度だけ消費する契約。** callbackのshared ownerがgesture、入力scope、native control transcriptを所有する。
前提確認で、Active中に`egui_winit::State`へ蓄積したraw Down/Upを、ROOTのEnterによるopenが先にpauseし、
同じStateを引き継いだ新deferred callbackが後からtakeする順序がコード上成立すると分かった。
native allocation／logical registration／contextは同じなので、callback時点のscopeだけでは旧操作の再所属を防げない。
そこでStateの実RawInput追加点でclient操作の開始を一度通知し、その時点で登録済みのParked input ownerだけがproofを発行する。
WM_TOUCHとpointer historyはwinitが正規化済みのWindowEventから受け、Win32側で別のtouch正規化を実装しない。
proofのopaque operation IDを`Event::PointerInputProvenance`として実Start直前へ置き、同IDのScopedNative proofと照合する。
並列Vecのindex／ordinalを使わないため、旧Stateの保留RawInputのappend、キーreleaseの先頭spliceでも旧入力が新proofを奪わない。
marker自体は入力操作でなく、単独／孤立markerやproof無し開始は権限を持たない。headless論理fixtureの例外は`cfg(test)`に限る。
本体からのobserver参照は既存path patchのegui-winitを直接依存するだけで、別版のegui graphを作らない。
scopeはwindow、context、viewport registration、Cの入力allocation epochである。
scope交代は旧gestureを即時終端し、旧touch recognitionを次batchの先頭で退役する。
未captureのnative pressも生成時scopeで照合し、旧scopeの操作を新contextへ付け替えない。
正常Up後のReleaseCaptureは取消と区別し、真のcapture lost／modal／touch Cancelは旧操作を終端する。
native controlとmodal／retireはgestureの同じ順序付きキューへ到着時に入れ、未captureの押下より取消が先へ移る二重キューを作らない。
raw Touchだけで昇格primaryがない操作も、既存correlationの読み取りpreviewで終端前に取消を対応付ける。
同callback frameのdiscard再passではgesture・commandを再消費しない。
touch correlationのApp frameとCallback frameは型で区別し、同じ数値による誤replayを防ぐ。
既存のframe全体ambiguous規則とcommand実行は変えない。別ownerの旧mouse入力が同batchにある場合も、
exact相関を通ってflush済みのtap完了factだけを現Canonical ownerがそのevent indexで消費し、復帰を一度確定する。
未flush／drag／pinch／cancelからfactを合成せず、一般のtouch command判定を緩めない。
transport mirrorと論理viewportのRawInput／IMEの終端は一括resetしない。
managerの既存intentが要求を所有し、rootの受理・commit両境界がregistration／allocation／context／Parked／modalを照合する。
commitはmediaを含め次rootのactive更新前の既存境界だけにそろえ、late callback受付後のpassive renderでは消費しない。
不在runtimeを遅着eventから再作成しない。新しいApp field、時間窓、retry、polling、追加repaintはない。
root wakeは既存event配送に含める。

**診断。** 通常ログ`[detached-activation]`に`boundary=normalized/normalized_release/normalized_discard/callback/callback_discard/accept/discard/commit`、型付きsource
`ClientClick / TouchTap / NonClientClick / NativeMedia`、型付きdecision
`Accepted / Committed / ModalOwned / NotParked / RetiredRegistration / GestureCancelled / CloseOwned / CommandOwned / ActivationFailed`、
window／registration／contextを1行で記録する。gesture終端・root受理／破棄・commit時だけ出し、frameやsampleごとには出さない。
command上書き・close・retireによる既受理要求の破棄もmanagerの所有境界で一度記録する。
正規化まで届いたがcallbackまで届かない場合も区別できる。破棄だけの診断はsourceで記録し、それだけのためにrootをwakeしない。
例（identityは説明用）:

```text
[detached-activation] boundary=accept source=ClientClick decision=Accepted window=1 registration=Some(...) context=Some(ViewerContextId(1))
[detached-activation] boundary=commit source=ClientClick decision=Committed window=1 registration=Some(...) context=Some(ViewerContextId(1))
[detached-activation] boundary=discard source=TouchTap decision=GestureCancelled window=1 registration=Some(...) context=Some(ViewerContextId(1))
```

**旧watcher testの対応。** polling専用helper／reducer／hit-test群は撤去し、release一回・focusだけ不可・
drag不可・modal取消・touch相関・replayをgestureの実egui入力試験、source同定・NC close優先・
native loop終端・退役／same-HWND reuse・兄弟不変をkey_inputのnative owner試験へ置換した。
旧content dragの復帰期待は今回の「dragでは復帰しない」決定に従って拒否へ変更した。
repair 3件はbackend adoptionの正本照合／ParkedLive維持／後継採用へ置換し、root wake＋兄弟不変試験も正規adoptionへそろえた。
`deferred_activation_queues_until_next_root_frame`はPDF再開・次root commitの期待を保ち、実deferred callbackのclickで要求を作る。
Aの赤2件はpoll controlだけを撤去し、実grid open→pause→兄弟close→実callback→同window/context復帰の期待を維持した。
さらにtap、drag、modal介入、active兄弟のbinding／page保持、bar close優先と、復帰click非再配送をscenarioで検査する。

撤去した21件（4つのsample helperは件数外）の対応は次のとおり。

| 旧watcher試験の期待 | 置換・維持する試験 |
| --- | --- |
| release一回、focusだけ不可、title/content drag | `coalesced_mouse_click_is_completed_once_even_after_layout_discard`、`held_mouse_waits_for_release_and_egui_rejects_an_out_and_back_drag`、`native_boundary_metadata_focus_and_keyboard_never_activate_without_a_click`、`realmachine50_parked_drag_does_not_activate_and_next_click_works` |
| bar close優先／outside、music slotと不正layout | `realmachine50_parked_bar_close_wins_over_client_activation`、`realmachine50_parked_coalesced_touch_closes_without_previous_bar_target`とgestureのclose対照、既存`parked_live_music_uses_music_chrome_close_slot`。旧watcherのclient closeはStart時に実controlを選ぶ同gesture ownerへ移す |
| native caption close優先、release outside／ready以前 | `native_detached_drag_maximize_and_close_have_one_owner`、`native_detached_cancel_ends_at_native_return_and_does_not_resume_on_late_up`。close成立のhit-test／release-outsideはOSの既存Def処理の責務とし、activation ownerはcloseを消費しない |
| 他HWND、重複rect、main／兄弟をrepairしない、foreground遅延 | Cの`realmachine50_equal_rect_replacement_keeps_keys_on_owning_hwnd`、`native_detached_target_block_retirement_and_same_hwnd_reuse_are_scoped`、backend adoption 3件。rect／foregroundの推測自体を撤去し、source registration／allocationを照合する |
| park起因押下の持越し不可、対象消失で旧押下取消 | `scope_retirement_precedes_an_already_queued_native_release`、`uncaptured_old_native_press_is_not_relabelled_but_new_click_in_same_batch_survives`、`stale_discrete_request_is_rejected_without_recreating_a_runtime`、`native_detached_sink_update_keeps_press_context_and_removal_is_terminal` |

最初のdeferred callbackにTouch Start／End／PointerGoneが同時に届く場合、hoverがなくバーは描かれず、
egui／既存bar touchの前pass widget targetもない。このためwidgetの常時宣言だけでは旧watcherのclose責務を
移せないことをsourceで確認した。独立レビューと合意し、復帰とcloseを同一gestureの排他的destinationへ
揃えた。描画条件・位置・shapeは変えず、pointer操作のwidget出力だけは二重producerにしない。
既存keyboard／accessibility factはeguiの型付きresponseから直接読み、pointer位置から推定しない。
page-overviewのcloseは通常バーとは実矩形が違うため、描画と同じ純geometry helperを共有する。
overviewの余白はcommand ownerに残し、実ボタンの端のcoalesced tapは同じcanonical close ownerで受ける。
active／ParkedLiveの既存overview closeと描画は変更しない。

終端監査では、Aの新logical targetがsink経由でShared／Contextを強保持するため、次のroot描画を
待たずterminal／manager dropで自分のexact registrationだけを退役する契約を追加した。
native allocationの再生成ではlogical targetを維持する。native key leaseへ保存する値も入力identityだけとし、
Aで追加したmutable gesture／sinkを余分に複製しない。これはAの参照所有の修正で、Cのbackend hookは変更しない。
独立実装レビュー（Sol / xhigh）は、終端参照所有・sinkのlock外解放・overviewの実矩形と入力元の分離・
fixtureの正規入力／immutable metadataの補足まで照合し、source上ACCEPT、未解決P1/P2なしとした。
レビュー担当はCargoを実行せず、自動検証の証拠は下表の実行結果とする。
関連する入力消費者も棚卸しし、raw event列のtake／append・キーreleaseの先頭追加・synthetic入力の末尾追加が
markerと実Startの対応を壊さないこと、modal等の既存retainは`InputState.events`側であることを確認した。
test-script用featureは製品の所有契約に使わず、同機能の変更も行っていない。

**変更ファイル（25件）。**

- 依存: `Cargo.toml`、`Cargo.lock`。
- 本体: `src/app.rs`、`src/key_input.rs`、`src/lib.rs`、`src/touch_correlation.rs`、`src/ui_fullscreen.rs`、
  `src/ui_fullscreen/page_overview.rs`。
- 閲覧owner／回帰: `src/app/detached_activation.rs`（新規）、`src/app/detached_activation_gesture.rs`（新規）、
  `src/app/detached_window_manager.rs`、`src/app/logic_service.rs`、`src/app/native_video.rs`、
  `src/app/viewport_retirement.rs`、`src/app/multiwindow_scenario_tests.rs`、`src/app/tests.rs`。
- vendor: `vendor/egui-winit/src/lib.rs`、`vendor/egui-winit/src/pointer_input_observer.rs`（新規）、
  `vendor/egui-winit/PATCHES.md`、`vendor/egui/src/data/input/event.rs`、`vendor/egui/src/response.rs`、
  `vendor/egui/PATCHES.md`。`vendor/eframe`は変更していない。
- 文書: 本書、`docs/detached-rework-plan.md`、`docs/architecture-overview.md`。

検証コマンド・件数・exit codeは本節末尾に記録する。実機のfocus／native loop／touch／HWNDでの動作と、
サブPCでのA入力到達の再確認は未実施。製品起動・確認用build・commitは利用者の指定により行わない。

検証はすべて指定`run-heavy.ps1`、Owner `codex/767-rm50-a`、WorkDir
`G:/home/mimageviewer-767s2`経由で直列実行した。共有全件ロック／slotを待ち、書込不可警告後は
runner指定どおりBelowNormal・6並列で継続した。待機中にソース点検のため止めたwrapper 2回は
Cargo開始前・0件・exit 1であり、テストの失敗・成功証拠には数えない。

| コマンド | 件数・exit code・証拠 |
| --- | --- |
| 本体の初回焦点nextest（下記filter） | **0件、compile failure、exit 101**。callback fixture 1か所のclose control引数不足（E0061）。実行開始前に判明し修正。`target/rm50-a-focused-compile-r1.log` |
| 本体の中間焦点nextest（同filter） | **81実行 / 77成功 / 4失敗、exit 100**。Bの既知2件と追加close fixture 2件。後者はcanonical proofを生成しないfixtureだったため、正規化APIを通す起点へ修正し、window／binding撤去の期待を維持した。A元赤2件・C1件は成功。`target/rm50-a-focused-r2.log` |
| 本体の中間回帰nextest（下記回帰filter） | **2,578実行 / 2,576成功（6 slow、2 leaky）/ 2失敗、exit 100**。同close fixture 2件だけの失敗。`target/rm50-a-regression-r2.log` |
| 中間 `cargo check -p mimageviewer --bin mimageviewer-core` / 引数なし `cargo fmt` | **ともにexit 0**。`target/rm50-a-check-r2.log`、`target/rm50-a-fmt-r2.log` |
| overview追加後の焦点nextest（同filter） | **84実行 / 80成功 / 4失敗、exit 100**。Bの既知2件と、直接itemsをseedしたoverview fixtureでimmutable page identity metadataを準備していない2件。既存overview fixtureと同じ`PreparedPageIdentityIndex::prepare`を補い、実open handler／snapshot種別／closeとownerの期待は維持した。この回は焦点の想定外失敗で止め、広域回帰／check／fmtは実行していない。`target/rm50-a-focused-r3.log` |
| `cargo nextest run --manifest-path vendor/egui-winit/Cargo.toml --lib --no-fail-fast` | **60実行 / 60成功 / 0失敗、exit 0**。`target/rm50-a-egui-winit.log` |
| `cargo nextest run --manifest-path vendor/egui/Cargo.toml --lib -E 'test(/non_pointer_click\|pointer_input_provenance\|pointer_cancellation\|viewport_retirement\|input_state/)' --no-fail-fast` | **32実行 / 32成功 / 0失敗、exit 0**。`target/rm50-a-egui.log` |
| 最終 `cargo nextest run -p mimageviewer --lib -E 'test(/realmachine50_\|detached_activation\|native_detached\|discrete_activation\|deferred_activation_queues\|callback_and_app_frame/)' --no-fail-fast` | **84実行 / 82成功 / 2失敗、exit 100**。A・Cおよび追加回帰は成功。失敗は今回対象外のB赤2件だけ。`target/rm50-a-focused.log` |
| 最終 `cargo nextest run -p mimageviewer --lib -E 'test(/detached\|multiwindow\|activation\|key_input\|native_focus\|viewport\|fullscreen\|touch_correlation\|native_video\|parked_live/)' --no-fail-fast` | **2,581実行 / 2,581成功（6 slow）/ 0失敗、exit 0**。281.982秒。`target/rm50-a-regression.log` |
| 最終 `cargo check -p mimageviewer --bin mimageviewer-core` | **core 1対象、exit 0**。既存警告あり。`target/rm50-a-check.log` |
| 最後のCargo操作: `cargo fmt`（引数なし） | **exit 0**。`target/rm50-a-fmt.log`。wrapperもexit 0（Bの既知2件だけを許容）。`target/rm50-a-root-wrapper.log` |
| 最終 `git diff --check` | **exit 0**。`target/rm50-a-diff-check.log`。改行コードの既存設定に由来するLF→CRLF警告のみ |

Bの失敗は`realmachine50_forwarded_open_without_dialog_retires_previous_live_owner`と
`realmachine50_forwarded_open_after_independent_export_retires_previous_live_owner`で、どちらも従来どおり
「forwarded open replaced the active session without retiring its old Depth owner」のassertである。
ignore・除外・期待の変更は行っていない。

描画条件・shape・layout・表示文言は変えていないため、`ui_snapshot`とUI glyph検査は今回の対象外。
全件gate、確認用build、製品起動、commit／pushは指定どおり実行していない。
未確認は実OSのfocus／non-client native loop／touchと、サブPCでのAの入力到達・復帰の再確認。
新しい通常ログを含む実機結果を、今回のheadless成功で代替しない。

#### A 独立実装レビュー指摘の対応（2026-11-14、第4回）

基点は `395ef758e`（C独立レビューP2対応済み）。`61495274a` を対象とした独立実装レビュー
（Sol / xhigh）のP2 2件・P3 1件を修正した。既存gesture ownerの責務を保ち、gestureファイル全体を作り直さない。
構造的な修正であることは設計担当と独立設計レビューで合意済み（BA-7）。Cのbackend lifecycle hook、
synthetic keyの終端権限、Bの外部openは変更していない。

- **P2-1／non-client button drag。** 旧実装は `WM_MOVING` / `WM_SIZING` の矩形変更だけでdragを終端し、
  caption buttonの外へ出て離す／外へ出て戻る操作を通常clickとして確定できた。
  同じnative ownerの `NonClient` 操作に押下hit-test対象とscreen座標を持たせ、同ownerの順序付きmotion / Upを照合する。
  最小化・最大化・help buttonでは対象逸脱または既存のclick距離超過を不可逆のDrag取消にする。
  NC messageのhit-test payloadを使い、captureされたclient / pointer messageではそのmessage座標を同じ確定HWNDへ
  `ClientToScreen` / `WM_NCHITTEST` で問い合わせる。owner lock外で問い合わせ、現在cursorやrect探索へ代替しない。
  client座標変換失敗は既存のログ＋通知と取消にとどめる。raw pointer IDと昇格mouseを分離し、別pointer／兄弟は操作しない。
  caption移動・resizeの既存矩形履歴、native close優先、`DefSubclassProc` のOS既定動作は維持する。
- **P2-2／touch強制無効。** 旧実装は診断switchでTouchFrameを空にして、raw Touchに対応する昇格Pointerのtap完了も失っていた。
  callbackのcorrelation / tap完了という入力factと、追加touch command／primary suppressionを分離した。
  `MIV_DISABLE_TOUCH_GESTURES` 設定時も生成時provenanceを持つ単指tapは復帰／closeを完結し、追加touch commandは実行しない。
  drag・複数接点・Cancel・modal・scope退役の取消、および一般App / Gridの診断無効処理は維持する。
  テストはprocess環境／production OnceLockを書き換えず、thread-localのRAII policy seamで同条件を再現する。
- **P3／取消ログのidentity。** 旧実装のdecisionには元scopeがなく、callbackが現在templateでログを再構築していた。
  既存gesture ownerへscopeの正本を集約し、native proof→実操作→queued decisionが元window / registration / context /
  native allocation epochを保持する。Sharedの重複scope fieldを撤去し、callbackはdecisionの元scopeだけを記録する。
  未captureの旧proofも診断metadataとして残すが、退役したadmissionは不可逆の `RetiredRegistration` とし、
  後から同じscope値が公開されても認可し直さない。生成時proofのない入力はscopeなしとして記録し、後継contextを代入しない。
  旧scopeのproofそのものを消すtest期待は、元identityを保持したまま権限を退役する期待へ変更した。
  拒否・二重消費防止のassertは維持し、P3で必要な理由を `NotParked` から `RetiredRegistration` へ正確にした。

回帰はoutside release、out-and-back、同button内の距離超過、raw pointer／promotion／兄弟、既存WM_MOVING dragを検査する。
強制無効のsame / split batch tap・bar closeはhelperと、正規化API→実deferred callback→既存service / root commitの
multiwindow scenarioでも検査する。後者は復帰clickのpage非再配送、DownだけではParked維持、close時のbinding終端とmain不変も要求する。
取消の元scopeはactive gesture・queued decision・未capture proof・同scope値への再入を型で検査する。
最終source差分は独立レビュー（Sol / xhigh）がACCEPT、blocking findingなしとした。レビュー担当はCargoを実行していない。

変更ファイルは `src/key_input.rs`、`src/touch_correlation.rs`、`src/app/detached_activation_gesture.rs`、
`src/app.rs`、`src/ui_fullscreen.rs`、`src/app/multiwindow_scenario_tests.rs`、本書、`docs/detached-rework-plan.md`。
vendor、描画・layout・表示文言を変更しないため、vendor crate／ui_snapshot／glyph検査は今回不要。
新しいApp field、待ち、retry、時間窓、追加repaintはない。製品起動・build-dev・全件gate・commit / pushは実行しない。

検証は全て指定 `run-heavy.ps1`、Owner `codex/767-rm50-a2`、同作業ツリー、重い処理は直列で実行した。
共有全件ロックを待ち、ロック書込み不可の警告後はrunner指定のBelowNormal・6並列で継続した。

| コマンド | 件数・exit code・証拠 |
| --- | --- |
| 修正前 `cargo nextest run -p mimageviewer --lib -E 'test(/native_detached_caption_button_\|native_detached_raw_caption_button_\|diagnostic_touch_disable_keeps_canonical_tap_and_close/)' --no-fail-fast --test-threads 6` | **5実行 / 0成功 / 5失敗、exit 100**。NC 4件と診断touch 1件の意図したassertで失敗。`target/rm50-a2-red.log` |
| 修正後 `cargo nextest run -p mimageviewer --lib -E 'test(/realmachine50_\|detached_activation\|native_detached\|discrete_activation\|deferred_activation_queues\|callback_and_app_frame/)' --no-fail-fast --test-threads 6` | **99実行 / 97成功 / 2失敗、exit 100**。今回の修正・追加回帰とA／Cの実機再現テストは成功。失敗は対象外Bの既知赤2件だけ。`target/rm50-a2-focused.log`、`target/rm50-a2-focused-junit.xml` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/detached\|multiwindow\|activation\|key_input\|native_focus\|viewport\|fullscreen\|touch_correlation\|native_video\|parked_live/)' --no-fail-fast --test-threads 6` | **2,602実行 / 2,602成功（4 slow）/ 0失敗、exit 0**。433.539秒。`target/rm50-a2-regression.log`、`target/rm50-a2-regression-junit.xml` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | **core 1対象、exit 0**。警告あり。`target/rm50-a2-check.log` |
| 最後のCargo操作: `cargo fmt`（引数なし） | **exit 0**。`target/rm50-a2-fmt.log` |
| 最終 `git diff --check` | **exit 0**。`target/rm50-a2-diff-check.log` |

焦点nextestの失敗2件は、上記A実装時と同じBのforwarded open赤テストである。JUnitでもその2件だけと照合し、
ignore・除外・期待の変更は行っていない。NC 4件と診断touch 1件の修正前赤テストは、修正後の焦点nextestですべて成功した。
実OSでのcaption buttonのcapture / hit-test／focus、mouse・touch・pen入力、サブPCでの再確認は未実施である。
headlessの入力配送／終端検査を実機確認の代わりにはしない。設計とコードの前提に実装を止める矛盾はなかった。

## 3. B — 外部openが旧独立ownerを退役させずsessionを置き換える

### 3.1 観測と原因

`log:2868–2869`（181.999s）でexportの入力保護解除→forwarded path解決が続き、
`:2900–2901`（182.103s）でwindow4をctx0へbindし、sessionを3→4へ直接置換している。
この間にwindow3のpauseはなく、window3 / ctx3のunbindは`:3901`（280.348s、mode変更時）までない。
「見えた窓はmainとwindow4だけ」「window4と次のwindow5でOtherWindow拒否」はサブPC担当の観測。

- `src/app/startup_ops.rs:303` のmodal中channel保留は機能している。
- mainでの通常leaf採用は `open_startup_file_if_visible`（`:1806`）→
  `open_loaded_file_fullscreen`（`:2434`）。後者は `fs_open_intent_from_grid=true` → `open_fullscreen` を直接呼ぶ。
- 通常gridの `src/app.rs:71955` `prepare_detached_context_for_grid_open` →
  `park_active_detached_context_for_new_grid_open`（`:71979`）を通らない。
  mainの一覧採用のcontent close（`:44399`→`:70645`付近）は、別のAtRest ownerをpauseしない。
- `prepare_viewer_presentation_open`（`:72196`）→ `begin_active_detached_session`（`:64813`）が新sessionを公開する。
  `active_viewer_context_id`（`:2994`）は新しいsessionのbindingを返すため、旧ctx3はAtRest / binding / Depthを持ったまま、
  activeにもpassive snapshotにも属さなくなる。
- 描画入口（`:69428`、`src/ui_fullscreen.rs:24383,27038`）の対象から旧窓が外れ、viewportは次のroot出力で
  維持されなくなる。backendのGC対象となることはコードから導けるが、実機のHWND破棄／非表示の正確な時刻は
  通常ログにない。正常なParkedへの退避ではなく、App側の孤児化である。
- `src/app/depth_view.rs:147` `depth_entry_allowed` の他context検査（`:154–160`）は、残ったDepthを正しく検出する。
  OtherWindowを弱める修正はしない。

再現したAは正しくParkedとsnapshotを持つ窓への復帰要求がない機構で、Bは離脱のhandoff自体がない孤児化である。
確認できた所有境界は異なり、共通のguard／Depth一括OFFでまとめて処理しない。
実機Aの入力到達が未観測である以上、実機の両件が同じ原因かを断定もしない。

複数ウィンドウの通常の物理静止画は、mainの一覧を残して別contextにする既存grid契約にそろえるべきで、
今回のctx0を新窓へ出す結果はその正常経路を通らなかった帰結である。
一方、メディア・DirectMedia等の既存ownerモデルを、この判定だけで一律に変更する設計にはしない。

### 3.2 失敗回帰

`src/app/depth_export_main_window_tests.rs` に追加する。
既存の独立ownerとReady Depth / exportのheadless fixtureを使い、実channelで外部画像を渡す。
modalありでは保留を確認し、実Escape→Cancel後、実workerのresolve / sidecar / openの採用まで進める。
modalなしでも同じ外部openを通す。targetの画像が開いた事前条件の後、以下を要求する。

- `realmachine50_forwarded_open_after_independent_export_retires_previous_live_owner`
- `realmachine50_forwarded_open_without_dialog_retires_previous_live_owner`

旧ownerのDepth終了・GPU publication除去、旧窓のsnapshotが1件でParked、新しい窓でOtherWindow拒否がないこと、
旧窓の再activation後にも拒否がないことを検査する。独立ownerの初期構成には既存test factoryを使い、
外部受付から採用は実handlerである。modal解除時にDepthだけをOFFする案を、modalなし対照が区別する。

### 3.3 非active化と同型入口の監査

| 経路 | 現在の確定点／修正上の扱い |
| --- | --- |
| 独立静止画pause | `pause_viewer_context_for_handoff` → `reset_detached_pause_foreground_modes`。所有Depthを終える |
| live-media park | `park_current_viewer_context_as_live_media_inner` → fork時に旧ownerの前景モードを終える |
| mainから独立窓へactivation | `retire_main_depth_for_independent_activation`。既存の課題3修正を維持 |
| passive再開 | `take_detached_snapshot_for_activation` / `park_detached_viewer_for_handoff`。捕捉済みsourceを先に処理する課題2修正を維持 |
| close | 明示idのterminal owner→`close_fullscreen_to_completion`→所有context退役。別contextをmountしただけでは終了しない |
| mode変更 | 残ったother contextも列挙してcloseする。今回ctx3がこの時点で消えることと整合 |
| F12 | `commit_viewer_session_deferred_feedback` がpair変更の所有Depthを終了する。nativeの保守完了は明示F12と区別済み |
| 外部openの通常main leaf | `StartupOpenPathOwner::InitialStartup / Activation` が `MainNew` を確定。実row・composition・sidecar・source leaseの採用可能性を確認した共通openでgrid handoffを一度行い、通常物理静止画は既存のmaterialized forkへ進む。mainの採用一覧を残す |
| Bookmark既存Cの継続 | typed Bookmark ownerとPreparing Cの既存leaseによる `ContextContinuation`。loaded leafもdescriptor継続も自己parkしない |
| Bookmark media新Cのinitializer | 予約済みloading C内の `ContextContinuation`。メディアの所有モデル・同窓再開を維持する |
| 表示対象外のDirectMedia | `DirectMedia { viewer_adoption }` → reader導入 → `RequiredFullscreen { viewer_adoption }` → `DeferredFsReopen`で同じ区分を保持。新規mainなら旧ownerをhandoffし、以後は既存のreader／basis分離条件へ進む。複数窓の静止画はreaderを独立Cへ移し、mainには保存済みbasisを戻す |
| フォルダ自動fullscreen | `MainListNavigation` → common adopterが返された正本hydrationへ区分を移す → `AutoFullscreenFolder`の実row採用で一度消費。明示leafをsidecar ownerが受理したときはauto page-choiceをそのexact要求へ置換し、二重openしない |
| PDF direct / warm | navigationまたは既存restore ownerが持つ区分を、移動前に同じPDFの `DeferredFsReopen`へコピー。cached rowsの即時採用・既存verificationの時点は維持し、最終ページ採用で一度消費する |
| ZIP・staged physicalの列挙 | 既存physical requestのnavigation由来を、その要求が作るauto reopenへ移す。composition／sidecar保留にも保持し、Cへ渡った継続は `ContextContinuation` |
| Collection physical finish → reopen | `CollectionNavigationRequest.origin.context_id / action`が既存Cを所有。`PreparedCollectionPhysicalNavigation::finish`とPDF deferredは明示 `ContextContinuation`で同じCを継続する |
| Smart childの独自install | 既存navigation由来をready adopterから独自 `DeferredFsReopen` constructorへ渡す。通常staged physicalを通らない枝でも失わない |
| ZIP内foreign archive変換提案 | 自動reopenを変換提案に移す際、auto boolだけに縮めず、その区分を新しい既存physical navigationへ移譲する。取消はhandoffしない |
| 未発見・hidden・拒否・取消・stale | 既存ownerで要求を終端し、新viewer公開のhandoffを行わない。別ページへ流用しない |

### 3.4 修正案（BA-7主、BA-5関連）

「3Dを持てるのはactiveな閲覧contextだけ」を意味上のactive ownerで守る。
scoped mount中かどうかやHWNDが前面かどうかで毎frame終了しない。

新しいmain側viewerの採用を、通常gridの既存handoffと共通の関数へそろえる。
requested rowが存在し採用できる境界で、既存typed ownerを使ってmainの新規openかC内の継続かを区別する。
旧ownerをpark／closeし、そのDepth終端・snapshot保存を完了してから新sessionを公開する。
modalは既存の最後の1件の保留を維持し、閉じた後に通常の採用へ戻す。新たな保留やrollbackは作らない。
新規main leaf、DirectMedia、container continuationは同じ「一度だけの所有移動」へ結び、
既存CのBookmark / loading continuationにはその移動を再適用しない。

session setter内の兄弟Depth一括OFF、OtherWindow検査のlive-only化、modal解除だけへのcleanup追加は採らない。
既存のmodal化は既に組合せを減らしており、今回足りないのは解除後の通常採用の所有移動である。
外部openを禁止する／操作を送り直させる簡素化は、受理した要求を閉じた後に開く既存仕様を削るため採らない。

### 3.5 B 実装記録（2026-11-14、BA-7主・BA-5関連）

基点は `92daa6e3c`。設計担当と独立設計レビュー（Sol / xhigh、2026-11-14）が構造的な修正に
合意した範囲で、外部openを通常gridと同じviewer採用へ接続した。§3.3の表は実装の確定版である。

**区分の正本。** `ViewerOpenAdoption::MainNew / ContextContinuation`を既存request/actionへ同居させる。
`StartupOpenPathOwner`が意味を持っている境界で確定し、履歴の `MainListNavigation`、common adopterの返す
`SidecarLoadContinuation`、exact leafのsidecar `DeferredFullscreen`、compositionの `FullscreenOpen`、
required／列挙の `DeferredFsReopen`へ保留・変換・再入を通して保持する。
`OpenRequestOwner::Navigation`や `MainListRequestStage::Continuation`のvariantだけでは分類しない。
main-list boundary receiptは一覧のsource close済みという既存の意味のままとし、viewer採用資格を混ぜない。
既存のgrid open・Bookmark／CollectionのC内継続は明示 `ContextContinuation`である。

**一度だけの確定。** 共通fullscreen入口でcomposition／sidecarの既存保留が終わり、実rowとsource leaseを
採用できることが分かった後、`MainNew`だけが `prepare_detached_context_for_grid_open` →
`park_active_detached_context_for_new_grid_open`へ進む。旧ownerのDepth終端・snapshot保存／closeを終えてから
既存の `route_materialized_physical_still_open_to_active_context` → session公開へ進む。
複数窓の通常物理静止画の新Cはmainとも旧ownerとも異なる。DirectMediaのreader／basis対、動画・音声の既存ownerは維持する。
区分は表示先設定や、その時点でmountされたsessionから再推定しない。
複数window設定を外した後も独立静止画Cが残り得るため、MainNewのhandoffをmodeだけで除外しない。
単一窓で通常のmain画像を開く場合は、既存seamがno-opとなりmainを再利用する。

EPUBのread leaseはpureな準備helperでlocalに保持し、handoff前のmain fieldへ公開しない。
handoff内の既存cleanupに消されず、fork先の既存取得が完了するまで生存する。非forkの場合だけ所属fieldへ採用する。
拒否は旧ownerの退役より前で終わる。新しいI/O、scan、完了待ち、App field／pending、rollback、retry、repaintは足していない。
まれな失敗は既定のログ＋通知に従う。

**同型経路。** DirectMediaのsidecar変換、PDF warm、staged ZIP/PDF、Smart childの独自reopen、
ZIP内変換提案のauto boolへの縮退を棚卸しし、§3.3の所有連鎖へそろえた。
明示leafとauto-folderが重なるときは、sidecarの同じownerでauto page-choiceをexact leafへ置換する。
hydrate後に一旦別の先頭ページを開いてから再handoffする組合せを作らず、指定leafの既存即時採用を保つ。
modalの最後の1件保留、native input／focus、A／Cの入力修正、DepthのOtherWindow判定は変更しない。

**診断。** 実sessionの差替え時に `[viewer-adoption]` の1行で `source=MainNew / ContextContinuation`、
`previous_window / previous_context / next_window / next_context`を記録する。保留・拒否だけでは差替えを記録しない。
生成元の区分は、再開後に現在のsessionから作り直さない。

**実行で判明した前提の訂正。** 当初の「DirectMediaの静止画はmain readerのまま」という実装側・独立レビュー側の
前提は誤りだった。`is_snapshot_active`はDirectMediaを意図的に除外し、既存のmaterialized／live forkはreaderを
実Cへ移してmainを保存済みbasisへ戻す。正本はdetached計画のDirectMedia fork記録、および既存
`visibility_direct_media_successful_fork_moves_owner_and_preserves_main_basis`等のtestである。
旧AtRest分岐がforkを迂回してmainへ直openした結果を、所有モデルの仕様として扱ってはならない。

handoffを正すと、この既存forkがpending中に `bump_items_generation` → projected owner getterを呼びassertした。
本番のTableOp列・pending中getter禁止は変えず、typedなlocal `DirectMediaMainRestore`でsource owner／basis世代／
origin／保存選択を捕捉し、`finish_fork`直後・incoming rating publication前にsourceの世代更新・observation再bind・
render／cover／cursorの復帰を完了させた。低level splitの直接呼出しも同じ完了処理を一度使う。
さらにrequired／auto-folder／reopenの選択確定をowner移動前へそろえ、fork後に消えたhidden indexをmainへ書き戻さない。
既存split直呼びtestを弱めず、本物の両production forkでmain世代・選択・観測、reader順と対応表、兄弟epochを検査する。
旧ownerなしの実外部openも同じ独立readerモデルを要求する。訂正とこの完了境界の修正も独立レビューが構造的修正としてACCEPTした。

**検証。** 全Cargo検証は指定 `run-heavy.ps1`、Owner `codex/767-rm50-b`、同作業ツリーで直列実行した。
修正前の `cargo nextest run -p mimageviewer --lib -E 'test(/realmachine50_forwarded/)' --no-fail-fast --test-threads 6`
は **5実行／2成功／3失敗、exit 100**（`target/rm50-b-red.log`）。元B2件と追加DirectMediaが旧Depth残存の同じassertで失敗し、
単一窓・未発見の対照は成功した。最終コマンド・件数・exit codeは下表に記録する。

修正後の初回焦点 `test(/realmachine50_/)` は **35実行／34成功／1失敗、exit 100**
（`target/rm50-b-focused-r1.log`）。元B2件・Folder／ZIP／fullfeature・Bookmark／取消／stale・A／Cは成功し、
DirectMediaだけ上記pending中getterで失敗した。`RUST_BACKTRACE=1`の同DirectMedia一件は
**1実行／0成功／1失敗、exit 100**（`target/rm50-b-direct-trace.log`）で呼出し列を確定した。
基点ソースにも同じfork内呼出しがあるが、基点でこの追加production fork testを実行した証拠はない。

次の焦点は **38実行／35成功／3失敗、exit 100**（`target/rm50-b-focused-r2.log`）。pending中assertは解消し、
新規fixtureの期待が残った。DirectMedia readerの`saved_items`等はmainへmove済みなので空であることが正本で、
reader実体2行・main basis1行の期待は維持したまま、保存配列の移譲も検査した。
Live forkの一覧選択は有効な可視行、表示中のhidden項目は`fullscreen_idx`で別に所有するため、
fixtureは前者0／後者2を設定しそれぞれ保持を要求する。materialized明示openは選択2を要求し続ける。
両訂正は独立レビューで既存契約への適正化と確認し、製品変更・検査の弱体化ではない。

最終検証（共通nextest引数: `cargo nextest run -p mimageviewer --lib -E 'FILTER' --no-fail-fast --test-threads 6`）。
共有全件ロックを待ち、ロック書込不可の警告後は指定runnerのBelowNormal・6並列で継続した。

| コマンド／FILTER | 件数・exit code・証拠 |
| --- | --- |
| `test(/realmachine50_/)` | **38実行／38成功／0失敗、exit 0**。B元赤2件と追加19件、A／Cの実機再現テストを含む。`target/rm50-b-focused.log`、`target/rm50-b-focused-junit.xml` |
| `test(/depth\|detached\|multiwindow\|startup\|forward\|bookmark\|collection\|direct_media\|sidecar\|grid_open/)` | **2,265実行／2,265成功／0失敗、exit 0**。282.318秒。`target/rm50-b-regression.log`、`target/rm50-b-regression-junit.xml` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | **core 1対象、exit 0**。警告あり。`target/rm50-b-check.log` |
| 中間 `cargo fmt`（引数なし） | **exit 0**。`target/rm50-b-fmt.log`。既存DirectMedia／registry testが指定filter外にもあるため、下記の追加検査後に最後のfmtを行う |
| `test(/direct_reader\|viewer_context_registry/)` | **109実行／109成功／0失敗、exit 0**。7.920秒。既存のbasis復帰・両fork・epoch／table契約を補完。`target/rm50-b-registry.log`、`target/rm50-b-registry-junit.xml` |
| 最後のCargo操作: `cargo fmt`（引数なし） | **exit 0**。`target/rm50-b-fmt-final.log` |
| 最終 `git diff --check` | **exit 0**。`target/rm50-b-diff-check.log` |

**変更ファイル（31件）。**

- 製品の採用／所有: `src/app.rs`、`src/app/startup_ops.rs`、`src/app/snapshot_ops.rs`、`src/app/sidecar_restore.rs`、
  `src/app/book_composition_permission.rs`、`src/app/collection_navigation.rs`、`src/app/smart_folder.rs`、
  `src/app/viewer_context_registry.rs`。`src/app/archive_inner_book.rs`と`src/app/close_return.rs`は既存C継続の区分明示。
- 新規／既存回帰と必須field対応: `src/app/depth_export_main_window_tests.rs`、`src/app/book_composition_permission_tests.rs`、
  `src/app/tests.rs`、`src/app/tests/startup_restore.rs`、`src/app/archive_inner_book_s2.rs`、`src/app/archive_inner_book_s3.rs`、
  `src/app/book_composition_revision_tests.rs`、`src/app/book_composition_runtime_tests.rs`、`src/app/book_resume_meter_tests.rs`、
  `src/app/rating_membership_tests.rs`、`src/app/similar_navigation_tests.rs`、`src/app/spread_shift_owner_tests.rs`、
  `src/app/tests/manual_nav_wrap_book.rs`、`src/app/tests/startup_notices.rs`、`src/filename_stack_ui.rs`、
  `src/ui_dialogs/epub_convert.rs`、`src/ui_fullscreen/continuous_quality_tests.rs`。後ろ3件はtest内のfield追加だけ。
- 文書: 本書、`docs/detached-rework-plan.md`、`docs/virtual-folders.md`、`docs/async-architecture.md`。

製品起動・build-dev・全件gate・commit／push・vendor変更は行っていない。描画・layout・表示文言は変更していない。
実OSの外部起動・focus／z-order、サブPCでの最終確認、PDFのnative描画は未確認。
PDF warmの追加検査は実resolver／loaderの即時行採用と保留ownerまでで、描画成功を意味しない。

## 4. C — rectによるキーsubclassの誤同定

### 4.1 原因とAとの区別

`src/ui_fullscreen.rs:25043` `install_key_input_subclass_for_viewport_rect` が、
`src/dwm_transitions.rs:143,383` の可視thread windowのrectスコアでHWNDを選ぶ。
実呼出しは `src/ui_fullscreen.rs:27985`。同じ位置・大きさの窓が2つあれば、所有viewportの判別材料はない。
`src/key_input.rs:3378` `install_window_subclass` は別viewportに登録済みなら`:3390`でconflictを記録し、
新しいHWNDへsubclassを付けない。拒否は正しく、誤って選んだproducerを直す必要がある。

実機のconflictは `log.prev:933`（69.860s）、`:6816`（304.080s）、
`log:2935`（182.149s、外部open）、ほかpart1にもある。
同rectへの交換は§6.4.5の意図した動作なので、今回の段階でこの既存誤同定を直す責任がある。
ただし交換後にD／↑が使えた後でpauseしたA1、交換自体がないA2の共通原因とは言えない。
conflictだけでは、その後ずっと新HWNDが未登録だったことも証明できない。

### 4.2 失敗回帰

`src/ui_fullscreen/native_focus_tests.rs` の
`realmachine50_equal_rect_replacement_keeps_keys_on_owning_hwnd` は、空regionを設定してから
`WS_EX_NOACTIVATE` / `SW_SHOWNOACTIVATE` で作った2つのtest HWNDを同rectにする。
画素・foreground切替・実入力・製品起動はなく、既存の非対話fake-input試験と同じ範囲である。
列挙順を固定せず、rectが選ぶほうを旧viewportに登録する。実subclass導入入口を通した後、
新HWNDへD、旧HWNDへ↑を `SendMessageW` で送り、各viewportに自分のedgeだけが来ることを要求する。
将来の正規HWND APIへの変更時は起点を置き換えても、この配送契約は弱めない。

### 4.3 修正案と必要な境界（BA-1）

HWNDの唯一の正本を、backendが所有するviewportの生成／再生成／破棄のlifetimeにする。
キーsubclassとdetached runtimeは同じ確定済みの対応を使い、geometryから推測しない。

- detachedだけなら、既存の `register_detached_window_hwnd_after_show` → runtimeの確定HWNDを
  キー登録へ共用できる。WindowId由来のViewportIdを使い、active／passive／再開で同じ対応を維持する。
- 通常Fullscreenも同じrect入口を使っている。これを含めて全childを正しくするには、
  正規のbackend lifecycle通知／参照APIが必要。backendのwindow mapが持つ
  生成時にそのnative allocationの `(ViewportRegistration, HWND)` を公開し、破棄前に同じallocationの入力登録を撤去する。
  Appにpendingを足さず、既存managerと入力registryのownerへ接続する。
- native allocationの寿命はegui registrationとは別である。
  eguiの `ViewportRegistration` はnative再生成でも残り得る（`vendor/egui/src/viewport.rs:1270`、
  `vendor/eframe/src/native/wgpu_integration.rs:2316`）。resizeでも変わるsurface generationも代用しない。
  まず旧allocationのsubclass／mapping／未配送edgeの撤去を、後継公開より前に同期完了させる契約を採る。
  これで旧終了の遅着との組合せを減らし、新しい世代tokenを当然には足さない。
  backendの既存allocation identityを渡す方式が必要なら、その必要性と終了順を実装前レビューで示す。
  Wgpu / Glow、immediate / deferred、root / childの生成・退役を同じ契約で扱う。
- 生存中の旧窓と新窓が同rectでも、両方の対応を同時に持てる。再生成ではnative allocationの寿命を区別し、
  旧HWNDの終了が後継登録やそのedgeを消さない。失敗時はログ＋通知で止め、rect再探索へfallbackしない。
- rect照合の除外追加、スコア調整、次frameのretry、同じHWNDを新viewportへ強制rebindする案は採らない。
  placement / focus / z-orderの計算・保存をこの修正に混ぜない。

**APIの調査結果:** `Frame::winit_window()` / raw handleはrootのframe由来で、子callbackへFrameは渡らない。
`ViewportRegistration` はlifetimeとViewportIdだけ、builder / event-loop hookは生成前でHWNDを渡さない。
`eframe::miv_test_script_window_witness` には正確なchildのhandleがあるがtest-script用のfeatureで、
製品の所有契約として利用しない。既存公開APIだけで全childの正確なHWNDを取得する経路は見つからなかった。
通常Fullscreenまでの正規hookはeframe側の変更を要する。**調査時点ではvendor変更禁止のため実装していない。**
次段の設計検収で、backend hookを実装範囲へ含めるか、まずdetachedの既存正本への共用に限定するかを明示決定する。
後者の場合、通常Fullscreenのrect経路が残ることを完了扱いに混ぜない。

### 4.4 C 実装記録（2026-11-14、独立設計レビュー反映）

設計担当の決定により、git管理下の `vendor/eframe` の変更を含む全 child 版を採用した。
独立設計レビュー (Sol xhigh) の P2 を反映し、撤去対象を subclass / 入力 registry / backend WindowId 配送対応 / allocation の未配送 native edge・gesture の4つとする。

`eframe::native_window_input` は Context の共有 registry と、backend が一つだけ所有する非 Clone の `NativeWindowAllocationLease` を持つ。
実 winit window の生成時に `(ViewportId, ViewportRegistration, WindowId, HWND)` を公開し、
AppCreator で一度接続する `NativeWindowInputListener` に ROOT の既存 allocation も一度通知する。
listener は mutex 外で入力 owner に lease を導入し、App / plugin を再入しない。
Wgpu の ROOT `init_run_state` と child `initialize_window`、Glow の ROOT 初期化と child を接続する。
immediate / deferred は両 backend の同じ native 生成点を通り、再生成・GC・suspend・終了も明示退役へ接続する。
Wgpuのsuspend経路はnative leaseと旧WindowIdの撤去に接続済みだが、ROOTの論理入力の保持は未対応である。
Windowsではwinitが`Suspended`を送らず当該helperもAndroid限定のため、独立レビュー（2026-11-14）のP2-2は設計担当判断で対象外として見送る。
生成が `initialize_window` だけを通るという前提は Wgpu ROOT に当てはまらず、別の ROOT 生成点も接続した。
独立実装レビューで、descriptor の `(registration, WindowId, HWND)` はすべて再利用され得るため、
値だけでは App の host cache の寿命を表せない点も確認した。backend lease だけが strong を持つ
Weak の `NativeWindowAllocationWitness` を query から参照し、明示退役開始時に同期失効する。
runtime の旧 HWND フィールドを `Absent / Backend(owner)`（test fixture は試験専用）へ置換し、
失効した host は `raw / alive / host_claim / placed` の既存境界で使えなくする。
同じ HWND 数値での再生成も、新しい owner の採用で既存 clear→set / host incarnation 更新を通す。
App に新しい flag / pending を足さず、配置・focus の計算や保存形式は変えない。

退役は successor の公開前に、backend が allocation lease を take → input lease が exact subclass を外す →
その owner の HWND ↔ viewport 対応と未配送 native edge / synthetic transport・gesture を終端 →
旧 WindowId の `viewport_from_window` を撤去（Glow は逆対応も撤去）→ 旧 native window / State を置換する。
Drop は同じ lease の冪等な最終手段で、`WM_NCDESTROY` や最後の `Arc<Window>` の解放を待たない。
RawInput / IME の retain は logical viewport の所有として従来どおり先に捕捉する。
既に egui に渡した synthetic Down だけは、入力 owner の終端を次の通常入力境界で一度消費する。
キーは正確な Up、Primary は操作を確定しない取消で終える。通常 Up は短時間の再生成で clicked / drag_stopped を
作り得るため、egui 側の所有者を指定した最小 cancel API に接続する（表示 consumer の guard は足さない）。
同じ logical registration の native 再生成では解放し、logical registration 自体が消滅した場合は resource も消えたものとして撤去する。
同じ ViewportId の新しい閲覧へ旧終端を持ち越さず、通常の physical held / IME / 兄弟入力は保持する。
layout の discard 再passでは元 RawInput のイベントが再配送されないため、旧終端も再適用しない。

`register_detached_window_hwnd_after_show` / `adopt_deferred_detached_window_hwnd_after_callback` は backend query を参照する。
watcher の観測 HWND もその viewport の正本と照合するだけで、候補を新しい正本にはしない。
入力用 rect 登録入口・before/after 差分・unclaimed 採用と、曖昧性回避のための「未確認 deferred は1窓/フレーム」を撤去した。
仮想デスクトップ同期・配置・raise 等の **入力以外の rect 利用は残す**。完了範囲は **入力 HWND 同定の rect 依存を解消**。
入力導入失敗はログと入力 owner の通知 queue → 既存 toast で通知し、その allocation の導入を終える。
物理 window の query は事実として維持するが、再導入・rect fallback・次 frame の retry はしない。

旧 geometry 候補分類の単体試験は、正規 backend factory の所有 lease から App の実採用を駆動する試験へ置き換える。
C の元の赤テストは同 rect の2つの実 test HWND に backend factory / listener を導入する入口へ変更し、
旧 HWND の↑と新 HWND のDがそれぞれ自分の viewport へ届く要求を維持する。
実 event loop / GPU を起動しない backend 試験は、共通 factory と両 backend の実 recreate helper / source lookup / State を検査する。
既存の両 backend headless lifecycle adapter に同じ正規 lease を載せ、実 GC・terminal Close dispatcher・
immediate callback 後の退役・終了用入力 helper についても query / source map / 一度だけの入力退役を検査する。
実 native 生成の root / child / immediate / deferred 到達はソース照合で、OS / GPU による全 lifecycle の実行証明とは区別する。

A / B の4件の赤テストはこの回の対象外である。

Cargo の検証はすべて `powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner "codex/767-rm50-c" -WorkDir G:\home\mimageviewer-767s2 -Command "<command>"` を経由し、直列で実施した。共有の全件ロック中は待機し、書込不可の警告後はスクリプト指定の低優先度・6 build jobsで継続した。

- 本体焦点: `cargo nextest run -p mimageviewer --lib -E 'test(/key_input|realmachine50_equal_rect|detached_backend_hwnd|watcher.*repair|late_post_show_hwnd|parked_live_hwnd_recreation|detached_placement/)' --no-fail-fast --test-threads 6`
  — **80 / 80 成功、exit 0** (`target/rm50-c-focused-final.log`)。
- egui: `cargo nextest run --manifest-path vendor/egui/Cargo.toml --lib -E 'test(/pointer_cancellation|viewport_retirement|input_state/)' --no-fail-fast`
  — **26 / 26 成功、exit 0** (`target/rm50-c-egui-final.log`、初回も26成功)。
- Wgpu: `cargo nextest run --manifest-path vendor/eframe/Cargo.toml --no-default-features --features default_fonts,wgpu --lib -E 'test(/native_window_input|viewport_input|viewport_retirement|child_close/)' --no-fail-fast`
  — **27 / 27 成功、exit 0** (`target/rm50-c-eframe-wgpu-final.log`、egui取消追補後の最終結果)。
- Glow: `cargo nextest run --manifest-path vendor/eframe/Cargo.toml --no-default-features --features default_fonts,glow --lib -E 'test(/native_window_input|viewport_input|viewport_retirement|child_close/)' --no-fail-fast`
  — **28 / 28 成功、exit 0** (`target/rm50-c-eframe-glow.log`)。
- 本体check: `cargo check -p mimageviewer --bin mimageviewer-core` — **exit 0** (`target/rm50-c-core-check-final.log`)。
- UI文字列: `python scripts/check_ui_glyphs.py` — 危険な glyph 0、exit 0。
- 本体回帰: `cargo nextest run -p mimageviewer --lib -E 'test(/key_input|native_focus|detached|multiwindow|viewport|fullscreen/) & not test(/realmachine50_/)' --no-fail-fast --test-threads 6`
  — **2,257 件中 2,256 成功・1失敗、exit 100** (`target/rm50-c-regression.log`)。
  失敗は `app::search_nav_entry_inventory_tests::search_nav_inventory_container_all_independent_key_inputs`。
  未変更の `src/app/search_nav_book_tests.rs:498`、collection fixture の `create_collection` 応答の
  `recv_timeout(5s)` が Timeout を返した。焦点80件では同じ試験が成功している。
  単独コマンド `cargo nextest run -p mimageviewer --lib -E 'test(=app::search_nav_entry_inventory_tests::search_nav_inventory_container_all_independent_key_inputs)' --no-fail-fast --test-threads 1`
  は **1 / 1 成功、exit 0** (`target/rm50-c-regression-isolated.log`)。
  ソース・fixture の待ち時間は変更していない。広い実行時の Timeout の原因は未特定で、負荷と断定しない。
  **整形後に同じfilter・6並列で再実行し、2,257 / 2,257 成功、exit 0**
  (`target/rm50-c-regression-same-conditions.log`, run `e3929d7a-7cfa-4193-b5e1-ec470e15bbce`)。
  単独成功との合算ではなく、この2,257件runを最終の指定回帰成功とする。初回Timeoutは同条件再実行でも再現しなかった。
- 依頼50: `cargo nextest run -p mimageviewer --lib -E 'test(/realmachine50_/)' --no-fail-fast`
  — **5件中 C 1成功、対象外の A/B 4失敗、exit 100** (`target/rm50-c-realmachine50.log`)。
  Aはflat/depthのpause後再activation、Bはmodal有無の外部openの旧前景owner退役の各2件で、期待・ignoreを変更していない。

途中の失敗と修正も残す。初稿の焦点 nextest は consumer enum の Drop と move の競合 E0509 で0件実行・exit 101、
core check は旧 RawInput.modifiers フィールド参照 E0609 で exit 101。
backend Wgpu 初回は27件中25成功・2失敗・exit 100で、headless immediate driver が実 prepare を省いていた点を直した。
本体焦点 v2 は旧配置 fixture 7箇所の HWND → witness の型移行漏れ E0308 で0件実行・exit 101。
v3 は80件中79成功・1失敗・exit 100で、watcher の passive 採用の期待を変換時に Opening にしてしまった点を、
元の Parked へ戻した（製品コードの変更なし、既存期待の維持）。最終焦点80件は上記のとおり成功した。
最後のCargoコマンドは引数なし `cargo fmt` — **exit 0** (`target/rm50-c-fmt-final.log`)。
最終の同条件回帰とfmtは、同じrun-heavy配下の `powershell -NoProfile -ExecutionPolicy Bypass -File target/rm50-c-final-gate.ps1`
で直列実行した（外側のログ `target/rm50-c-final-gate.log`、exit 0）。
`git diff --check` — **exit 0**（CRLF変換の警告のみ）。最終UI glyph検査も危険0・exit 0。
独立実装レビュー (Sol xhigh、2026-11-14) は最新の2,257件run、最終fmt、整形後差分、初回失敗との分離を確認し、
**Cスコープ受入可・追加P1/P2なし・記録不一致なし**と判定した。

変更ファイル:

- 本体: `src/lib.rs`, `src/key_input.rs`, `src/app.rs`, `src/app/update_phases.rs`,
  `src/app/detached_window_manager.rs`, `src/app/detached_placement.rs`, `src/ui_fullscreen.rs`。
- 本体テスト: `src/app/tests.rs`, `src/app/detached_media_exchange_tests.rs`,
  `src/ui_fullscreen/native_focus_tests.rs`。既存の配置試験も正規 backend factory の lease を保持して駆動する。
- eframe: `vendor/eframe/src/lib.rs`, `vendor/eframe/src/native_window_input.rs`,
  `vendor/eframe/src/native/wgpu_integration.rs`, `vendor/eframe/src/native/glow_integration.rs`,
  `vendor/eframe/src/native/viewport_lifecycle.rs`, `vendor/eframe/PATCHES.md`。
- egui: `vendor/egui/src/context.rs`, `vendor/egui/src/input_state/mod.rs`,
  `vendor/egui/src/context/pointer_cancellation_tests.rs`, `vendor/egui/PATCHES.md`。
- 文書: 本書、`docs/detached-rework-plan.md` §11、`docs/architecture-overview.md`。

実装中の独立レビュー (Sol xhigh) は、host cache の allocation 寿命と、synthetic Primary の
通常 Up / layout 再passでの再終端を P2 とした。Weak owner と runtime enum、正規 cancel と一度だけの
終端へ修正した差分の再レビューでは追加 P1/P2 なし。最終検証・整形後も上記の受入可を確認した。

未確認: 製品は起動していない。実event loop / GPUを伴うROOT・childの生成、実機focus / z-order、
F11・仮想デスクトップ・OS API失敗注入は今回実行していない。両backendの実生成点への接続はソース照合、
実HWNDのキー配送と撤去は空regionの非対話fixture、GC / Close / immediate / deferred / shutdownは共有の実helperを通すheadless試験で確認した。
描画・レイアウトは変更していないため `ui_snapshot` は実行していない。広い回帰時のDB fixture Timeoutの原因は未特定で、
単独成功を合算して指定回帰をgreenとは扱わない。独立レビューの照合を受け、整形後に同じ2,257件filter・6並列を再実行し、
全件成功を確認した。初回非ゼロと単独成功の履歴も別証跡として保持する。初回Timeout自体の原因は未特定である。
`test-lib-full.ps1` / `test-full.ps1` / `build-dev.ps1`、製品起動、commit / pushは実施していない。

追加のwinit境界監査（ソース照合、製品変更なし）: lock中の確認では Windows の `WindowId` はHWND由来で、
atomicな一意IDではない（winit 0.30.13 `platform_impl/windows/mod.rs:101`, `window.rs:645–646`）。
`Window::drop` は専用messageをPostして破棄する（`window.rs:1107–1114`, `event_loop.rs:2445–2447`）ため、
通常の同一handler内recreateでは後継生成時に旧HWNDがまだ存在する。Glowのcurrent描画窓はArcも保持する。
この通常経路で旧buffered eventが同値WindowIdの後継Stateへ渡る確定例は確認されなかった。
一方、winitのreentrant queueはWindowId付きEventのFIFOで、一般のallocation tag / purgeは持たない
（`event_loop/runner.rs:208–253`）。任意のnested native pumpと数値再利用の組合せまで一般保証したとは扱わず、
実機での配送確認は未実施として残す。通常経路で成立する証拠のない組合せのため、新しい待機・世代機構やwinit改変は追加しない。

### 4.5 C 独立実装レビュー指摘の対応（2026-11-14）

基準`dc0546d27`への独立レビュー（Sol / xhigh）のP2-1を、A後の`61495274a`で確認した。
syntheticキーのDownは終端権限を登録する一方、physical Downでその権限を手放さず、
後からnative allocationが退役すると旧Upが物理保持を解除していた。既存
`terminal_synthetic_input_is_consumed_once_before_physical_takeover_and_real_discard`は、
takeover後に退役するbranchのキー登録を省いていた。両branchで登録すると、次の空RawInputで
`key_down(ArrowRight)`がfalseになることを確認した（1実行 / 0成功 / 1失敗、exit 100、
`target/rm50-c2-red.log`）。

修正は既存の`SyntheticLogicalGesture`とone-frame prepared transportの所有境界へ集約した。
元RawInputのphysical KeyDownが同じlogical registration／keyのsynthetic保持を`Releasing`に移し、
その入力hookの先頭で旧Upを一度消費してからphysical Downへ渡す。物理Downはfresh pressのままになり、
以後のallocation退役はそのキーのUpを発行しない。別key／別registration／IMEは変更しない。
通常timelineのUpも現存する所有権だけを消費し、repeatは所有の継続に限定して再取得に使わない。
初回Downも既存physical保持へ割り込まない。producerの初回／repeat／Upの事実を
`PreparedSyntheticKey`へ残し、eguiへ渡す`event.repeat=false`の従来規則は維持した。
拒否した入力はprepared transportから除去し、discardで元のphysical eventが消えても再取得しない。
新しいApp field、待ち、時間窓、再試行、repaint、native配送の変更はない。

既存のsynthetic専用fixtureも、同じContext／実logical registrationを継続して使う前提へ揃えた。
modifier順序の対照はphysical Aとsynthetic Enterを使い、今回禁止した同じkeyの共有を前提にしない。
元のイベント順・modifiers・配送数のassertは維持する。追加5件は実plugin＋discardの4件と、
兄弟registrationの所有を確認する1件で、takeover→再生成、無権限repeat／Up、既存physical保持、
同frame physical tapを検査する。
レビュー中の「値が等しい旧Upと新Upをまとめて削除する」P2は、受理列そのものをcacheへ保持する形へ
修正し、旧Up→新Down→新Up＋実discardで両passのidleを固定した。
独立ソースレビューはACCEPT、追加P1/P2なし（担当は編集・Cargo未実行）。

P2-2は設計担当判断どおりコードを変更していない。lockfileのwinitはregistry版`0.30.13`で、
その`src/platform_impl/windows/`全体に`Suspended`／`suspended`の発行箇所はない。
Windowsの`event_loop/runner.rs:355`は初回`Resumed`だけを送る。
`Suspended`のproducerはAndroid `mod.rs:218`、iOS `event_loop.rs:202`、
Web `event_loop/runner.rs:230`にあり、API解説`application.rs`のplatform別記録とも一致した。
eframeの`wgpu_integration.rs`の`drop_window`／`recreate_window`も`cfg(target_os="android")`である。
実装前の`452eda75b`にもROOT viewport全体のremoveがあり、Cで新たに導入した挙動ではない。
上流へ戻す際の論理入力保持の未対応点を`vendor/eframe/PATCHES.md`へ明記した。

検証は指定`run-heavy.ps1`、Owner `codex/767-rm50-c2`、WorkDir `G:/home/mimageviewer-767s2`で直列実行した。
レビュー修正のため型検査中に中断した1回は0件・exit 1であり、テスト失敗には数えない
（`target/rm50-c2-compile-interrupted.log`）。書込不可警告後はrunner指定のBelowNormal・6 build jobsで続けた。

| コマンド | 件数・exit code・証拠 |
| --- | --- |
| 修正前 `cargo nextest run -p mimageviewer --lib -E 'test(=key_input::tests::terminal_synthetic_input_is_consumed_once_before_physical_takeover_and_real_discard)' --no-fail-fast` | **1実行 / 0成功 / 1失敗、exit 100**。両側のsynthetic key登録を補った状態で再現。`target/rm50-c2-red.log` |
| 最終 `cargo nextest run -p mimageviewer --lib -E 'test(/key_input\|synthetic\|realmachine50_\|native_focus\|detached/)' --no-fail-fast --test-threads 6` | **630実行 / 628成功（4 slow）/ 2失敗、exit 100**。125.050秒。追加5件・既存回帰の両branch・A/Cのrealmachine50回帰は成功。失敗は今回対象外の既知B2件のみで、JUnitでも完全名を照合した。`target/rm50-c2-focused.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | **core 1対象、exit 0**。既存警告あり。`target/rm50-c2-check.log` |
| 最後のCargo操作 `cargo fmt`（引数なし） | **exit 0**。`target/rm50-c2-fmt.log`。外側の`target/rm50-c2-wrapper.log`は既知B2件を許容してexit 0 |

Bの失敗は`realmachine50_forwarded_open_without_dialog_retires_previous_live_owner`と
`realmachine50_forwarded_open_after_independent_export_retires_previous_live_owner`。
いずれも従来の旧Depth owner未退役のassertで、ignore／除外／期待の変更はしていない。

変更ファイルは`src/key_input.rs`、本書、`docs/detached-rework-plan.md`、
`docs/architecture-overview.md`、`vendor/eframe/PATCHES.md`。
実機のkeyboard／IME／native再生成は未確認。製品起動・確認用build・全件gate・commit／pushは行わない。
描画・UI文言は変更していない。

## 5. 退行の確認と補足2件

ソース比較では、Aのwatch/callback経路は `53d224a5c` より前から同じ。
`9044445e6` のui_fullscreen差分はテスト追加で、`01c9b2195` はnative requestの独立性、
`178a3ece2` はBuildingのpair保持である。Aの当該入力経路を変えた証拠はない。
Bのstartup_opsは `01f5e898f..HEAD` の差分がなく、v5.2.0にも同じ直接openがある。
Cのrect入力登録も旧版に残っている。§8の同じ5テストは現版・`01f5e898f`で同じ境界の失敗になった。
再現した欠陥は少なくとも指定比較基点に存在し、`9044445e6` / `01c9b2195` / `178a3ece2`で新しく導入されたものではない。
`01f5e898f`は課題2・3の`53d224a5c`を含むため、この実行だけで同commitより前を比較したとは扱わない。
同commit以前のA関連経路、v5.2.0の外部open／rect入口の確認はソース比較であり、tagでの同テスト実行ではない。
実機Aの原因・導入時期の断定は引き続きできない。
旧版に同じ欠陥があることと、新しい同rect交換で露出頻度が増えた可能性は区別する。

- **手順8:** `part1/log.prev:5113–5115`（544.612s）でwindow9はctx0に新規bindしVideoで開始、
  556.697sに同じctx0 / window9でImageに変わる。設定と新規openの経路から連動窓と判定する。
  独立へのfork／parkを挟まないF12 ONの連動メディア→画像は§6.4.5の同じホスト再利用であり、交換(a)の対象外。
- **手順15補足:** `src/app.rs:72774–72795` の既存one-shotは、viewer内のメディア送りで今のpresentationを維持する。
  main fullscreenから↓で動画へ進むとmain fullscreenのままなのは従来仕様。
  同じ処理と2026-07-02の理由コメントがv5.2.0にもある。いずれも今回変更する案を含めない。

## 6. 診断（初回調査の案、A実装で導入）

初回調査では案だけを記録した。依頼50 Aでは、以下の入力診断を製品の通常ログへ実装した。
最終的な型・出力境界・例は§2.4を参照。session差替えの診断はBの実装範囲に残る。

通常ログで不足しているのは、クリックの受領から復帰要求の受理までの相関である。
既存の所有window / viewport registration / contextと、型付きの入力元・理由を、受理／破棄／commit時に記録する。
例は `ActivationIngress::{ClientClick, NonClientClick, NativeMedia}`、
`ActivationDecision::{Accepted, ModalOwned, NotParked, RetiredRegistration, GestureCancelled}`。
frameごとの推測や8msごとの全sample loggingではなく、実際の入力／所有遷移の境界だけを記録する。
session差替えには旧／新のwindow・contextと採用元（main leaf / C内継続）を併記する。
既存のrequest / event ownerへ情報を持たせ、Appのbool / Option / pendingを増やさない。

Aの実機入力がどの境界に到達したかはこのログを含む次段の実機確認で検証する。
本稿の失敗回帰が通ったことだけを、実機Aの解消証明にはしない。
まれな失敗は既定で割り切り、ログ＋通知とする。回復用のretry／保留／失敗分類の機構は設計しない。

## 7. 規模と実装前の判断

| 作業 | 目安（レビュー・焦点回帰を含む） |
| --- | --- |
| C: detached正本への共用だけ | 1〜2人日。通常Fullscreenまでの完了ではない |
| C: 正規backend lifecycle hookと全childの登録・退役 | 3〜5人日。vendorを含む次段の範囲承認が必要 |
| A: client / non-clientの離散入力→既存intentへ集約 | 2〜3人日。Cとの接続、drag / touch / modal / replay対照を含む |
| B: accepted main openとC内継続のhandoff統一 | 1〜2人日。DirectMedia / folder / PDF / ZIPの追加検証を含む |
| 合算 | 全child版なら6〜10人日程度。実機検証の待ち時間は別。Aの実機原因が別なら再調査分は別 |

実装前に、(1) Cのbackend変更範囲、(2) Aのnon-clientを含むproducer対応表、
(3) Bの既存typed owner別の一度だけのhandoff点を独立レビューで確定する。
根本の所有構造を優先する利用者方針に対する推奨はCの全child版である。
detached限定を先に行う場合も、通常Fullscreenのrect依存まで解消した完了とは扱わない。
この調査段階では製品実装に進まない。

## 8. テスト実行記録

Cargoはすべて指定 `run-heavy.ps1`、Owner `codex/767-realmachine`、
WorkDir `G:/home/mimageviewer-767s2` 経由で直列とする。重い処理は本担当で重ねない。
共有slotが使用中ならそのまま待ち、書込不可の警告時はrunnerの低優先度実行を続ける。
既存検証と同じjobs=1 / incremental=0 / MSBuild node reuse無効を使用する。

焦点コマンドは `cargo nextest run -p mimageviewer --lib -E 'test(/realmachine50_/)' --no-fail-fast --retries 0`。
旧版は同じコマンドに `--manifest-path .\target\767-gate-old-01f5e898f\Cargo.toml` を加える。
旧版のsrc全1,036 blobをGitの`01f5e898f`と照合した後、現行と同じ3テストファイルの差分だけを移植し、
残る1,033 blobが旧版と一致することを検証した。manifestの依存path補正とnative資産の実体コピーは
前回の比較用snapshotを再利用し、product source・root vendorは変更しない。
証拠は `target/767-realmachine50-tests.patch`、`target/767-realmachine50-old-source.log`。
初回fixture修正後にも同じpatchを旧版の3ファイルへ適用した結果との完全一致を確認した
（`target/767-realmachine50-old-port-proof.log`、exit 0）。

初回実行は **5実行 / 0成功 / 5失敗、exit 100**（compile9分05秒、実行1.981秒、
`target/767-realmachine50-current-r1.log`）。Bの2件とCは狙った欠陥で失敗した。
Aの2件は最初のviewportを描く前に次を開いたためsnapshotを作れず、Parkedの事前条件で失敗した。
最初の窓を実driverで描画してからpauseするfixtureへ直し、期待は変えなかった。

| 版・コマンド | 結果／証拠 |
| --- | --- |
| 現コード `d96e47c0b`（製品 `178a3ece2`）、上記nextest filter | **5実行 / 0成功 / 5失敗、exit 100**。compile8分41秒、実行1.653秒。`target/767-realmachine50-current.log`。全件とも狙った境界での失敗 |
| 旧コード `01f5e898f`、上記filter＋`--manifest-path` | **5実行 / 0成功 / 5失敗、exit 100**。compile8分57秒、実行1.937秒。`target/767-realmachine50-old.log`。全件とも同じ境界で失敗し、fixture準備の失敗はない |
| `cargo fmt`（最後のCargo操作、引数なし） | **exit 0**。`target/767-realmachine50-fmt.log` |
| fmt前後の3テストファイルのSHA256照合 | **3/3一致、exit 0**。`target/767-realmachine50-source-hashes-after-fmt.log`。実行したtest sourceからの変更なし |
| `git diff --check` | **exit 0**。`target/767-realmachine50-diff-check.log`。既存LF→CRLF警告のみ |

現コードの失敗出力（各ペアで同じ内容）:

```text
A: assertion `left == right` failed: the paused viewport received a complete click,
   even though neither OS sample contained its down edge
     left: None
    right: Some(1)

B: forwarded open replaced the active session without retiring its old Depth owner

C: replacement viewport must receive D from its own HWND while an older HWND
   has the same rectangle; edges=[]

Summary [1.653s] 5 tests run: 0 passed, 5 failed, 17720 skipped
```

旧版もAは `left: None / right: Some(1)`、Bは同じ旧Depth残留、Cは同じ `edges=[]` で失敗した。
旧版のsummaryは `5 tests run: 0 passed, 5 failed, 17698 skipped`。

`#[ignore]` は付けず、製品未修正のまま失敗を残す。
全件gate、検証用build、製品起動、commit / pushは行わない。

## 9. 独立レビューと引継ぎ

独立 GPT-6.1 Sol / xhigh が6ファイルの差分、原因／限界の分離、修正提案と現／旧の全証跡を照合し、
**今回の調査・設計提案・赤テストは受入可、未解決P1 / P2・記録不一致なし**と判定した。
Cのnative allocation寿命の明確化指摘は、後継公開前の同期撤去を優先する契約として反映した。
未確定root causeについての bounded GPT-6 Astra / medium の読み取り監査でも、Aを実機原因確定とは扱わないこと、
backendのclass / callback更新に別の確定原因が見つからないこと、×だけではcallback到達を示さないことを確認した。
両レビュー担当は編集・Cargo・製品起動を行っていない。

この受入は製品実装の着手承認ではない。§7のA入力producer表・B typed ownerごとの確定点・C backend範囲は、
次段の設計担当の検収と独立設計レビューで確定する。
未確認は実機Aの入力到達／watch拒否の瞬間、旧HWND破棄の実時刻、DirectMediaや全containerの同型実再現、
実OSのfocus / DPI / touch / non-client操作を含む修正後の実機動作。今回のheadless結果をこれらの代わりにしない。

変更は3テストファイルと本書、`depth-parallax-plan.md` §6.4.6 / §17.19、`detached-rework-plan.md` §11の6ファイル。
製品コード・ログ・vendorは未変更で、`feat-767-stage2` / HEAD `d96e47c0b`を維持する。
