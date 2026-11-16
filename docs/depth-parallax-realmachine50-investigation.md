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

2026-11-15の利用者決定「案1」により、以下の初回記録のdrag拒否は§5.7の完了時復帰へ改訂した。
通常canvasのprimary／単指touchとcaption／resizeは押下から終端まで復帰ownerが消費し、
ページ送り／パンへ再配送しない。close、caption buttonの対象外取消、真のOS取消、modal／退役は維持する。

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
未flush／pinch／cancelからfactを合成せず、一般のtouch command判定を緩めない。
復帰用の単指完了factは§5.7でdragも含める形へ改訂し、通常tapの距離／時間規則とは分離した。
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
drag終端・modal取消・touch相関・replayをgestureの実egui入力試験、source同定・NC close優先・
native loop終端・退役／same-HWND reuse・兄弟不変をkey_inputのnative owner試験へ置換した。
初回はcontent dragを拒否したが、2026-11-15の利用者決定「案1」により完了時復帰へ変更した。
repair 3件はbackend adoptionの正本照合／ParkedLive維持／後継採用へ置換し、root wake＋兄弟不変試験も正規adoptionへそろえた。
`deferred_activation_queues_until_next_root_frame`はPDF再開・次root commitの期待を保ち、実deferred callbackのclickで要求を作る。
Aの赤2件はpoll controlだけを撤去し、実grid open→pause→兄弟close→実callback→同window/context復帰の期待を維持した。
さらにtap、drag、modal介入、active兄弟のbinding／page保持、bar close優先と、復帰click非再配送をscenarioで検査する。

撤去した21件（4つのsample helperは件数外）の対応は次のとおり。
この対応表の試験名は初回時点の記録で、drag拒否を改訂した現在の回帰は§5.7を参照。

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
  この時点の「外へ出て戻っても取消」は依頼55の設計担当訂正で置き換えた。
  現在は解放対象だけを照合し、戻って対象上で離した操作は完了する（§5.8）。
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

## 5. 依頼55の対応（2026-11-15）

基点は `25f3f9e41`。実機の観測者はすべてサブPC担当。
`C:/miv-subpc/results-55/notes.md` と `logs/part2a`、`logs/part3`、`logs/part1` の
`mimageviewer.log` を照合した。以下のheadless結果を実機での修正確認とは扱わない。

### 5.1 A: winitとegui-winitのcursor寿命不一致をソースと赤テストで確認

**観測。** part2aの窓4は281.456秒のnormalized Press、281.457秒のRelease／callback、
281.552秒のcommitが揃う。300.456秒の2回目pause後はclient行がなく、394.117秒のNonClient取消は
同じregistration `866D`／lifetime12を指す。窓2も2回目pause後に同症状となる。
part3では兄弟窓の復帰でpauseした後にもclient復帰が複数回成立している。

**検査した境界。** 本体の `realmachine55_client_resume_survives_repeated_main_focus_close_and_grid_open` は
正規native allocation registry／入力leaseと既存ScenarioDriverで、実grid open、pause、callbackの
生成時proof、既存transport、root commit、実 `reconcile_fullscreen_after_main_focus` を3往復する。
兄弟open／closeの対照は `realmachine55_client_resume_survives_repeated_sibling_open_and_close`。
既存兄弟の交互再開で相手をpauseする対照は `realmachine55_client_resume_survives_repeated_sibling_reactivation`。
初回App試験はnative sourceの生成・退役とnormalized入力を合成していた。追加修正では
実Stateの共通pointer変換APIへCursorMovedを一度与え、その後はCursorLeft→CursorEntered→
MouseInput Down／Up（再入時のCursorMovedなし）を与える。実observerが生成したproofとRawInputを
callbackへ渡し、同座標再入で3往復しても同window／context／pageと兄弟の資源を保持する。
native sourceの生成・退役は合成し、実backend配送／OS focusの証明とはしない。
別にeframeの既存Wgpu／Glow lifecycle adapterで、同childのImmediate／Deferred切替、
別main fullscreenのImmediate生成／明示退役を3往復し、child／兄弟の同leaseとWindowId対応を検査する。
egui-winitの実State変換・raw drain／observerにも反復する別main viewport退役の対照を追加した。
backend adapterはOS windowを作らず、State変換は別試験である。

**ソース照合と赤テストで確認した故障機構。** `State::on_mouse_button_input` はpointer位置がない押下を
raw PointerButtonへ変換せず、`pointer_input_observer::normalized_input` はその押下を通知しない。
winit 0.30.13のWindows `event_loop.rs` はWM_MOUSELEAVEでCursorLeftを送る一方last_positionを消さず、
同座標のWM_MOUSEMOVEではCursorMovedを省略する。egui-winitはCursorLeftで位置を失うため、
出たときと同じclient座標へ戻ると位置なしのまま押下を失う。vendorの実State／observer試験で
この列を赤化した。設計担当は同じソース照合によりこれをAの修正対象と確定した。
実機でこのイベント列を採取した証拠はなく、part3との差を座標の違いで説明する点も未確認である。

WindowsのStateは最後のCursorMovedの物理座標を保持し、CursorLeftでは従来どおりlogical位置を
消してPointerGoneを出す。CursorEnteredで保持位置を現在の実効pixels_per_pointへ変換し、
通常のon_cursor_movedと同じPointerMoved生成経路へ戻す。実際の位置が変わっていれば後続の
CursorMovedが上書きする。保持値はwinitのlast_positionとCursorMovedごとに一致し、
touchのEnd／Cancelでは消さず、新しいnative allocationのStateはNoneから始める。
root／child全Windows viewportに適用し、根拠のない他platformのEntered復元は追加しない。
WindowId routing／native leaseの生成・退役は変更せず、前回の押下時診断もすべて残した。

| 入口 | 型付き理由 | 記録するidentity |
| --- | --- | --- |
| 両backendの実WindowId配送直後 | `NativeWindowInputDiscardReason::UnmappedWindowId` | backend、実WindowId。viewportを推定しない |
| Stateのmouse生成境界 | `PointerInputDiscardReason::PointerPositionUnavailable` | 物理source、viewport、現在registration |
| observer呼出し境界 | `PointerInputDiscardReason::ObserverNotInstalled` | Mouse／Touch(id)と入力元、viewport、現在registration |
| mIV observer closure／entry照合 | `MissingViewportRegistration` / `RegistrationNotInstalled` | 入力phase/source、実viewport、照合registration |
| mIV native binding | `Unbound` / `GestureOwned(実gesture)` / `InputStateUnavailable` | 入力phase/source、実viewport、registration |

すべてprimary押下の境界だけで通常ログに1行出し、毎frame・motion・releaseでは出さない。
observer再設置・rect探索・入力再送はしない。

### 5.2 B: captionの同期native操作の終端を同じownerで確定

captionのMouse Down／DoubleClickが `DefSubclassProc` のmodal処理へ入った場合、
そのnative callが返るまで同じownerが操作を所有する。call内のWM_CAPTURECHANGEDでcaptureが0へ
返却される事実を外部取消と混同せず、call return（またはWM_EXITSIZEMOVE）で完了する。
foreign HWNDへのcapture移譲、focus／cancel／Escapeは取消を維持する。
利用者決定「案1」によりcaption／resizeの移動は取消にせず、native操作の終端で復帰する。
旧initial_rectの捕捉・GetWindowRect・移動量比較・8px閾値は撤去した。
caption buttonは解放時のhit-test対象が押下対象と違う場合だけ取り消す。
途中で外へ出ても戻って対象上で離せば完了する（独立レビュー後の設計担当訂正、§5.8）。
同じbutton内の大きな移動だけでは取り消さない。×のCloseOwnedとOSのDefSubclassProcは維持する。

### 5.3 C: 復帰だけは保持時間をクリック条件に使わない

mouseのActivateOnlyが `released_buttons_with_click_counts` のSomeに依存したため、
eguiのクリック時間上限（既定0.8秒）を超えるとDragへ誤分類された。
touchも既存TouchCommandのtap時間（700ms）に依存して長押しを取り消していた。
利用者決定「案1」によりmouseの復帰用距離判定／press_posを撤去し、同gesture ownerのreleaseで
復帰する。client外やcloseの上で離しても、ActivateOnlyで開始した操作はcloseへ付け替えない。
capture中のmouse PointerGoneは終端とせず、真のcapture loss等の取消は維持する。
touchは既存単指ownerとexactな昇格pointer相関からsingle_contact完了factを取り、
距離・保持時間を復帰の条件にしない。一般のtouch command／tap時間、バーcloseの既存click／tap規則は変更しない。
したがって窓内のtouch長押しも同じ復帰仕様にでき、今回の既存command所有との矛盾はない。
`callback_discard` は `decision=GestureCancelled` に加えて `reason=Drag` 等の実際の
`ClientActivationCancelReason` を同じ1行へ出す。元scopeのidentityは維持する。

### 5.4 D: ParkedLiveへNonClient ownerと既存transportを接続

`DetachedActivationRequest` が生成時の `DetachedActivationTargetState` を保持し、
sourceがNonClientだからParkedと推定する規則を撤去した。targetはParked／ParkedLiveを対象にする。
ParkedLiveの実描画枝には従来Shared transportがなく、pruneもそれを除外していたため、
既存Sharedを同じsnapshotの寿命で生成・保持する。新しいApp pendingは作らない。
NonClientの完了だけを既存transport→intent→root commit→`resume_parked_live_media`へ渡す。
ParkedLiveの窓内クリックは既存NativeMedia producerが所有し、新しいclient復帰producerを競合させない。
復帰／close後は既存target退役とpruneでSharedを解放する。

### 5.5 外した候補・設計合意・検証

- 同registrationのNonClientが届くため「旧allocationだから一律破棄」は全体説明にならない。
  最終成功clickのReleaseも揃い、NC開始後もclient無反応が続くので、ClientPressedの単純な残留だけでも
  説明できない。ViewportId衝突候補も設計担当の既存照合で除外されている。
- これらの除外は「backend mapも正常」「Stateに位置がある」という証拠にはならない。
  新しい診断と実機イベント列で切り分ける。Aをheadlessで再現できたとは記載しない。
- B／C／Dは同操作のownerと終端を揃える構造修正であり、独立設計レビュー（Sol / xhigh、2026-11-15）も
  BA-7主／BA-1関連の修正として合意した。時間による待ち、追加repaint、App状態、入力再試行は不要。
  全停止窓の一括resetや再生成は、既存閲覧の所有と表示を壊すため採らない。
- 2026-11-15の利用者決定「案1」で、通常canvasのprimary／単指touchとcaption／resizeのdragは
  操作終端で復帰と確定した。既存HUD／右ドラッグ／ページ一覧等のcommand ownerは維持し、
  通常tapの認識規則や複数接点・OS取消を復帰操作へ変換しない。

検証は指定 `run-heavy.ps1`、Owner `codex/767-rm55`、同WorkDirで直列実行する。
ロック場所への書込不可警告後も、runnerのBelowNormal／6 build jobsのまま継続した。

修正前の `test(/realmachine55_caption|realmachine55_parked_live|realmachine55_parked_long/)` は
**7実行／1成功／6失敗、cargo exit 100**（`target/rm55-red.log`）。Bのmodal終端2件、
Cのmouse／touch長押し2件、Dのadmission／target2件が失敗し、drag取消の対照は成功した。
初回焦点試験は **132実行／131成功／1失敗、exit 100**（`target/rm55-focused-r1.log`）。
Aの反復main focus guard／兄弟open-closeも成功した。失敗した旧テスト
`native_cancel_between_old_down_and_new_click_preserves_the_new_click` は、operationを持たない
テスト専用Native列が取消を次Start直前へ配置し、その前の新PointerMovedが距離取消を先に作るもの。
製品と同じcanonical proof／ScopedNativeへ起点を揃え、CaptureLostという取消理由の期待は維持し、
元scope・fresh click成功・翌frameの非再生を検査した。製品の取消順を変更するための対処はしていない。

最終結果（nextestはすべて `--no-fail-fast --test-threads 6`）。件数は各コマンドの実行数で、
焦点と回帰の重複を合計していない。

| コマンド | 件数・exit code・証拠 |
| --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/realmachine55_\|realmachine50_\|detached_activation\|native_detached\|non_client\|pointer_input_observer/)'` | **133実行／133成功／0失敗、exit 0**、10.363秒。`target/rm55-focused.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/realmachine50_\|realmachine55_\|detached_activation\|native_detached\|non_client\|pointer_input_observer\|parked\|native_video\|touch\|key_input/)'` | **715実行／715成功（4 slow）／0失敗、exit 0**、99.168秒。`target/rm55-regression.log` |
| `cargo nextest run --manifest-path vendor/egui-winit/Cargo.toml --lib` | **61実行／61成功／0失敗、exit 0**。物理sourceの診断補足後の最終結果は `target/rm55-egui-winit-final.log` |
| `cargo nextest run --manifest-path vendor/eframe/Cargo.toml --no-default-features --features default_fonts,wgpu --lib -E 'test(/realmachine55_\|viewport_retirement\|native_window_input/)'` | **21実行／21成功／0失敗、exit 0**。`target/rm55-eframe-wgpu-final.log` |
| 上と同じeframeコマンドで `--features default_fonts,glow` | **21実行／21成功／0失敗、exit 0**。`target/rm55-eframe-glow-final.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | **core 1対象、exit 0**。警告あり、`target/rm55-check.log` |
| 最後のCargo操作 `cargo fmt`（引数なし） | **exit 0**。`target/rm55-fmt.log`。vendorは各manifestの `cargo fmt` もrun-heavy経由で先行実施 |

AのApp反復3件、両backendのnative source保持、State変換の対照はすべて成功し、
**初回時点ではAの実機症状はheadlessで再現できなかった**。追加修正では§5.1／§5.7の
同座標再入による故障機構をソース・vendor赤テストで確認した。実機part3の座標差との対応は未確認である。
B／C／Dの赤テストと追加対照、既存NativeMedia、A／Cの
realmachine50回帰はすべて成功した。独立source review（Sol / xhigh）は最終の所有・終端・
診断metadata・canonical fixtureとA対照の補足までACCEPT、追加P1/P2なし。

**変更ファイル（19件）。**

- 本体: `src/app.rs`、`src/app/detached_activation.rs`、`src/app/detached_activation_gesture.rs`、
  `src/app/detached_window_manager.rs`、`src/key_input.rs`、`src/touch_correlation.rs`、
  `src/touch_input.rs`、`src/ui_fullscreen.rs`。
- App層の反復テスト: `src/app/multiwindow_scenario_tests.rs`。
- vendor: `vendor/eframe/src/native/{wgpu_integration,glow_integration,viewport_lifecycle}.rs`、
  `vendor/eframe/src/native_window_input.rs`、`vendor/eframe/PATCHES.md`、
  `vendor/egui-winit/src/{lib,pointer_input_observer}.rs`、`vendor/egui-winit/PATCHES.md`。
- 文書: 本書、`docs/detached-rework-plan.md`。

実OSのcaption modal loop／ダブルクリック最大化・touch・focusは未確認。
静止画Parkedのdoubleclickはnative ownerと共通transport／commitの組合せで守り、
静止画のOS操作そのものを実行した証拠とはしない。製品起動、全件gate、確認用build、commit／pushは行わない。

### 5.6 初回依頼50時点の退行の確認と補足2件

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

### 5.7 利用者決定「案1」とAの追加修正（2026-11-15）

追加実装の基点は `e9c909f56`（前回A〜Dの修正を設計担当がcommitしたもの）。
利用者の決定（統括経由、2026-11-15、案1）により、通常canvasのprimary／単指touchは
クリック・長押し・dragをすべて操作終端で復帰に使う。captionのclick／drag／double-click、
最大化・resizeもOS操作終了時に復帰する。入力列は終端までそのownerが消費し、
同操作でページ送り／パンを行わず、次の通常操作から閲覧へ届ける。
×は従来のCloseOwned、caption buttonは押下対象の外へ出た場合の取消を維持する。
modal／退役・capture loss／OS取消・複数接点の取消も既存の所有契約を維持する。
既存HUD／右ドラッグ／ページ一覧等のcommand ownerは変更せず、通常canvasの復帰producerを競合させない。

**Aの根本修正。** §5.1のWindows cursor寿命不一致を直し、物理座標は実CursorMovedだけから保持する。
Stateのlogical位置とwinitの物理履歴を同じものとして消していた点が誤りである。
CursorLeftは従来のPointerGoneを維持し、Enteredで保持位置を現在倍率へ復元する。
touchのlogical清掃では物理mouse履歴を消さず、別native allocationでは引き継がない。
`on_pointer_event` はWindow不要の正規pointer変換APIとして既存private helper／observerを共有し、
Appの反復テストも実State変換→observer→provenance→callback→intent→root commitを通す。
normal `on_window_event` のAccessKit／IME trace／observerの順序は維持した。

**client／captionの簡素化。** mouseの復帰用press位置・距離helper、NCのinitial_rect捕捉・
GetWindowRect・移動量閾値とgeometry取消を撤去した。触れたものはplacementの保存計算ではない。
ActivateOnlyのclient外release／close上releaseは開始destinationを維持し、caption buttonはhit-test対象の外へ
一度出れば戻っても取り消す。同じbutton内の大移動は完了とする。
touchはstationary完了factをsingle_contact完了factへ改め、一般のtap時間／距離やcommand実行を変えず、
raw End後のexactな昇格pointer tailが揃ったときだけ復帰する。

**ParkedLive native動画のtouch。** 既存native mouseはDown／Up所有で既にdrag・長押しを終端復帰へ
渡せたが、native canvas touchは通常recognizerへ流れ、長押し・drag Endを復帰に使えなかった。
描画側のdimmed状態で新しく分類する案は、control／raw入力が別queueのためActive時のStartを
Parked後に再所属させる。これを採らず、native HWNDの入力生成境界で既存key_input bindingを捕捉する。
通常入力／ActivateOnly／既存command／抑止を既存NativeTouchOwnershipの排他的routeとし、
source epoch、正規logical hostのallocation、操作tokenをEndまで照合する。
media窓のOS ownerが0でも、placementの変換前に確定済みlogical hostを使い、親探索はしない。
sink生成時に正規親allocationのwitnessを捕捉し、後継の同HWNDをStart時に採り直さない。
既存pumpのVisible入力epochをstate採用直後・効果実行前にread-only projectionへ公開し、
native子窓のepochと既存output cancel tokenもStart／Endで照合する。
これにより旧presenterの後段event gateやreceiver dropを直接入力経路が迂回する穴を閉じた。
旧scopeが失効したときだけ新child／outputのfresh操作を開始でき、旧tailはoperation identityで拒否する。
正規F12の親変更は既存pump switchで新sinkを作り、旧outputのDropは既存cancel→shutdownへ進む。
request全体へ新しいproofや保留を増やす代わりに、これら既存ownerの寿命を参照する。
source交換／modal／focus・capture loss／CancelMode／Escape／destroy／multi-contactは元tokenを終端する。
rendererへActivateOnlyを渡さず、通常touch command／panoramaへの副作用を出さない。
完了は既存Shared transportへNativeMediaのwindow／context復帰intentとして渡す。
End後の同C内の通常メディア送りは完了済みintentを取り消さず、context／window／registrationの
差替えは既存admissionが拒否する。新しいApp pending・時間窓・retry・repaintは追加しない。

独立設計レビュー（Sol / xhigh）は、Aのcursor寿命、drag終端、native touchの生成時ownerを
根本境界の構造修正として合意した。完成source reviewも親allocation／native子窓／output寿命・
source交換・各取消・fresh操作と旧tailの分離までACCEPT、追加P1/P2なし。

**追加検証の途中記録。** すべて同じ `run-heavy.ps1`／Owner `codex/767-rm55` で直列実行した。

- drag仕様の赤: `test(/realmachine55_.*(drag|caption|resize|mouse_release)|native_detached_drag_maximize|native_detached_caption_button_far_motion|held_mouse_and_out_and_back_drag|touch_drag_completes_but_multiple/)` は
  **16実行／2成功／14失敗、exit 100**（`target/rm55-drag-red.log`）。
- Aの赤: vendorの `test(/realmachine55_cursor_|realmachine55_recreated_state|realmachine55_pointer_adapter/)` は
  **7実行／4成功／3失敗、exit 100**（`target/rm55-reentry-red.log`）。同座標復元・倍率変更・touch終端後が失敗した。
- 初回追加焦点は **146実行／143成功／3失敗、exit 100**（`target/rm55-drag-focused-r1.log`）。
  native canvas touch Endから復帰する実handlerが未接続の赤1件と、次click／tapのfixture座標誤り2件。
  後者はLTRの先頭ページで前ページ側を押していたためで、製品変更はせずactual modeのforward側へ直し、
  items2／page0／paged、touch PageSideをassertしたうえでpage1への期待を維持した。
- output cancel参照の最終補完前は **233実行／233成功、exit 0**
  （`target/rm55-drag-focused-before-output-cancel.log`）。最後の補完には同親の旧／新outputを扱う回帰を追加した。

**追加修正の最終結果。** nextestはすべて `--no-fail-fast --test-threads 6`。
焦点／回帰の件数には重複があるため合計しない。全Cargo操作は指定run-heavy経由で直列実行した。

| コマンド | 件数・exit code・証拠 |
| --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/realmachine55_\|realmachine50_\|detached_activation\|native_detached\|non_client\|pointer_input_observer\|native_touch\|touch_correlation/)'` | **234実行／234成功／0失敗、exit 0**、13.336秒。`target/rm55-drag-focused.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/realmachine50_\|realmachine55_\|detached_activation\|native_detached\|non_client\|pointer_input_observer\|parked\|native_video\|touch\|key_input\|native_window/)'` | **770実行／770成功（1 slow）／0失敗、exit 0**、95.816秒。`target/rm55-drag-regression.log` |
| `cargo nextest run --manifest-path vendor/egui-winit/Cargo.toml --lib` | **69実行／69成功／0失敗、exit 0**。`target/rm55-drag-vendor-winit.log` |
| `cargo nextest run --manifest-path vendor/eframe/Cargo.toml --no-default-features --features default_fonts,wgpu --lib -E 'test(/realmachine55_\|viewport_retirement\|native_window_input/)'` | **21実行／21成功／0失敗、exit 0**。`target/rm55-drag-vendor-wgpu.log` |
| 上と同じeframeコマンドで `--features default_fonts,glow` | **21実行／21成功／0失敗、exit 0**。`target/rm55-drag-vendor-glow.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | **core 1対象、exit 0**。警告あり。`target/rm55-drag-check.log` |
| `python scripts/check_ui_glyphs.py` | **危険なglyph 0、exit 0** |

最後のCargo操作 `cargo fmt`（引数なし、同run-heavy経由）は **exit 0**
（`target/rm55-drag-fmt-final.log`）。`git diff --check` も **exit 0**。

**今回の変更ファイル（16件）。**

- App／入力: `src/app/detached_activation.rs`、`src/app/detached_activation_gesture.rs`、
  `src/app/detached_window_manager.rs`（テストのkind補完）、`src/app/multiwindow_scenario_tests.rs`、
  `src/key_input.rs`、`src/touch_correlation.rs`、`src/touch_input.rs`。
- native入力: `src/video/native_touch.rs`、`src/video/native_window.rs`、`src/video/native_window_pump.rs`。
- vendor: `vendor/egui-winit/src/lib.rs`、`vendor/egui-winit/src/pointer_input_observer.rs`、`vendor/egui-winit/PATCHES.md`。
- 文書: 本書、`docs/detached-rework-plan.md`、`docs/architecture-overview.md`。

前回A〜Dを含む `25f3f9e41` からの変更は23ファイル。前回のnative allocation lease／WindowId退役、
NonClient／長押し／ParkedLive transport、今回のcursor寿命とdrag／native touchを合わせて検証した。
未確認は実機の同座標イベント列とpart3の座標差、OS caption modal loop／最大化・resize・touch・focus。
製品起動・全件gate・確認用build・commit／pushは行わない。描画・レイアウトは変更せず、ui_snapshotは実行していない。

### 5.8 依頼55の独立レビュー指摘の修正（2026-11-15）

基点は `1d99a7a87`。独立レビュー（Sol / xhigh）の P1・P2 と設計担当の
caption button 指摘を扱う。案1の「復帰に用いた操作は閲覧へ再配送しない」、
modal / 退役 / CloseOwned の既存規則を維持する。構造変更への合意は継続し、
実装時にも別の Sol / xhigh が生成・配送・取消・終端と同frame後続入力を監査した。

**P1: 位置なしの物理releaseを失っていた。** winit 0.30.13 の
`src/platform_impl/windows/event_loop.rs` の `capture_mouse`（978行付近）は
SetCaptureで窓外入力も同じ窓へ配送する。WM_MOUSEMOVE（1643行付近）は
signed client座標を取得し、`get_pointer_move_kind`（2652行付近）が外へ出たと判定すると
CursorLeftを出し、その後に位置が変わっていれば窓外座標のCursorMovedを出す。
`mouse.last_position` はこのMoveで更新する。一方、WM_MOUSELEAVE（1712行付近）は
CursorLeftだけを出し、last_positionを消さない。したがって窓外Moveの後に届くLeaveで
egui-winitのlogical位置が再びNoneになり得る。WM_LBUTTONUP（1797行付近）は
`release_mouse`（985行付近、ReleaseCapture）を先に実行し、位置なしのMouseInput Releasedを出す。
UpのlParamからCursorMovedを補わず、last_positionも更新しない。
これはregistry内の当該版ソースとの照合であり、実機で全通知順を記録した証拠ではない。

eguiのPointerGoneはhoverを消すだけでdownを解除しない。位置必須のPointerButtonへ
過去の座標を代入すると、実際には窓外で離した操作を古い座標のclickにできてしまう。
observerだけReleasedに変えても、eguiの保持とcallbackのraw入力終端が残る。
このため `Event::PointerButtonReleased { button, modifiers }` を位置なしの終端として導入した。
egui-winit Stateは実MouseInputでforwardしたbuttonの保持を所有し、その同じStateの
位置なしUpを一度だけこのeventへ変換する。touchのpointer昇格は物理mouse保持を作らない。
新Stateは保持を継承しない。focusだけでは実Upの配送権を捨てず、App側の既存取消が
その操作の復帰を拒否する。simulate_touch_screenのraw Touch Endには、Touchの既存契約である
「最後に検出した位置」を渡すが、mouse releaseの位置やhoverには転用しない。

eguiは新eventでdown / press_originを終え、click=Noneのreleaseを記録し、位置を作らない。
observerはその実終端をReleasedとして通知し、callbackは同じ順序のraw eventでActivateOnlyだけを完了する。
closeのhitは認可しない。grid / front pointer / music / flat・panorama navigator / depth / touch correlationの
release ordinal、canvas取消、modal終端保持、syntheticとの混在判定も同じeventへ接続した。
前回の手作りUpテストを根拠にはせず、実State→observer→deferred callback→root commitを通す
窓内・窓外・外から戻ったreleaseの3件を追加した。どれもページ・panの不変と次の通常操作を検査する。

**P2: 診断が製品sinkへ届かなかった。** log facadeの最大levelがWarnであり、
dependency filterにもvendor診断の許可がなかった。次の2targetのInfoだけを明示的に許可し、
facade最大levelをInfoにする。一般のvendor Info、近似target、Debugは引き続き通さない。

- `egui_winit::pointer_input_observer::diagnostic`
- `eframe::native_window_input::diagnostic`

producerの押下境界・理由型は維持し、毎frameの出力は追加しない。GPU診断の所有lock照会は
対象Errorだけへ短絡させ、一般Infoに共有lockを増やさない。
回帰は別test processで通常loggerを初期化し、実Stateの位置なし押下・observer未設置押下から
log facade→非同期writer→通常log fileまで到達することを検査する。
eframeのexact targetと、一般vendor Info拒否・既存Tantivy / surface Warn・GPU起動時Errorの対照も含む。

**caption button: 解放時の対象とOSの選択を使う。** motionだけでは取り消さず、
同じownerのUpのhit-testが押下対象と一致すれば完了候補にする。外で離した場合だけ
CaptionButtonOutsideとし、外へ出て戻って対象上で離せば完了する。
WM_NCLBUTTONUPはcapture中にはpostされないという
[Win32仕様](https://learn.microsoft.com/en-us/windows/win32/inputdev/wm-nclbuttonup)がある。
capture中の[WM_LBUTTONUP](https://learn.microsoft.com/en-us/windows/win32/inputdev/wm-lbuttonup)を、
既存の同thread [WH_GETMESSAGE hook](https://learn.microsoft.com/en-us/windows/win32/winmsg/getmsgproc)で
modal loopの消費前に観測し、そのmessageの座標をclient→screen→WM_NCHITTESTへ渡す。
これは新しい位置推定・rect探索ではない。
[WM_NCPOINTERUP](https://learn.microsoft.com/en-us/windows/win32/inputmsg/wm-ncpointerup)のhitと
pointer capture時のWM_POINTERUPも同じownerで扱う。

[WM_SYSCOMMAND](https://learn.microsoft.com/en-us/windows/win32/menurc/wm-syscommand)の
実dispatchで押下対象に対応する最大化・最小化・復元等が選ばれた事実を
`ButtonActionCommitted`へ確定し、native callの返却まで所有する。
確定後のOSによるfocus / capture副作用では選択を巻き戻さない。
確定前のtrue cancel、modal、registration退役、CloseOwnedは引き続き優先する。
SCだけがqueueから見えた段階では確定しない。
公開仕様が保証するcapture / hook / commandとrelease対象を合わせた設計であり、
実Windowsのbutton tracking内部をheadlessで再現したという意味ではない。
Upがhookにも一切見えずcapture0取消が先に届く列は従来どおりCaptureLostとし、
未検証のOS列を推測で復活させる状態は足さない。

**検証記録。** 全Cargo操作はOwner `codex/767-rm55` の指定run-heavy経由、直列。
修正前のmain焦点は9実行 / 3成功 / 6失敗、exit 100（`target/rm55-r1-red-main.log`）。
窓外release・sink・外戻り2件・SC完了2件が狙った境界で失敗した。
vendorの実State窓外releaseも1実行 / 0成功 / 1失敗、exit 100
（`target/rm55-r1-red-vendor.log`、Pressed→Cancelledと期待Releasedの不一致）。
途中の焦点は254実行 / 253成功 / 1失敗、exit 100（`target/rm55-r1-focused.log`）。
新規touch回帰が、実mouse混在でCurrent touchの実行権を落とす既存規則とViewer geometryに対して
accepted grid tapを期待していたfixtureの誤りだった。製品規則は変えず、raw ordinal 1を消費した後の
ordinal 2の新mouse press / releaseがMouse所属であることと、touch commandが空の対照へ直した。
さらに3件の実State試験をzoom=2で強化した後の焦点は254実行 / 251成功 / 3失敗、exit 100
（`target/rm55-r1-focused-final.log`）。復帰・ページ・pan不変までは成功し、次の通常クリックにpage1を
期待した点が既存の「拡大時clickはview操作」という仕様に合わなかった。`fullscreen_click_belongs_to_view`
を照合し、拡大したまま次の新dragでpanできる期待へ直した。製品変更は追加していない。

最後の焦点は **254実行 / 254成功 / 0失敗、exit 0**
（`target/rm55-r1-focused-green.log`）。位置なしreleaseの下流共有consumerを含む。
loggerのignored child helperは通常sinkのglobal初期化を隔離するため、親testが別processで明示実行する。

vendorの最初の緑確認は73実行 / 72成功 / 1失敗、exit 100
（`target/rm55-r1-vendor-winit.log`）。従来の「位置なしUpは、forward済DownがあってもCancelled」
という旧期待を、新しい終端契約へ合わせた。Down未受領時のUpはCancelled、
forward済Downの位置なしUpだけは一回のReleased、という両方を同じテストで検査する。

**最終結果。** nextestの共通引数は `--no-fail-fast --test-threads 6`。
下表の件数は重複があるため合算しない。全行exit 0。

| コマンド（nextestは上記共通引数を付与） | 件数／exit code／証拠 |
| --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/realmachine55_\|realmachine50_\|detached_activation\|native_detached\|non_client\|pointer_input_observer\|native_touch\|touch_correlation\|canvas_gesture/)'` | **254 / 254成功、exit 0**。`target/rm55-r1-focused-green.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/realmachine50_\|realmachine55_\|detached_activation\|native_detached\|non_client\|pointer_input_observer\|parked\|native_video\|touch\|key_input\|native_focus\|canvas_gesture\|similar_preview\|sidecar_restore\|navigator\|overflow\|grid_reclick_open\|logger/)'` | **1,113 / 1,113成功（1 slow）、exit 0**、138.201秒。`target/rm55-r1-regression.log` |
| `cargo nextest run --manifest-path vendor/egui-winit/Cargo.toml --lib` | **73 / 73成功、exit 0**。`target/rm55-r1-vendor-winit-gate.log` |
| `cargo nextest run --manifest-path vendor/egui/Cargo.toml --lib -E 'test(/positionless_release\|pointer_cancellation\|pointer_button_target\|input_state/)'` | **17 / 17成功、exit 0**。`target/rm55-r1-vendor-egui-gate.log` |
| `cargo nextest run --manifest-path vendor/eframe/Cargo.toml --no-default-features --features default_fonts,wgpu --lib -E 'test(/realmachine55_\|viewport_retirement\|native_window_input/)'` | **21 / 21成功、exit 0**。`target/rm55-r1-vendor-wgpu.log` |
| 上行と同じコマンドで `--features default_fonts,glow` | **21 / 21成功、exit 0**。`target/rm55-r1-vendor-glow.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | **core 1対象、exit 0**、349警告。`target/rm55-r1-check.log` |
| `cargo fmt --manifest-path vendor/egui/Cargo.toml`、同 `vendor/egui-winit/Cargo.toml`、同 `vendor/eframe/Cargo.toml` | **3コマンド、各exit 0**。`target/rm55-r1-vendor-format-*.log` |
| `python scripts/check_ui_glyphs.py` | **危険なglyph 0、exit 0** |

最後のCargo操作は引数なし `cargo fmt`（同run-heavy経由、**exit 0**）。
その後の `git diff --check` も **exit 0**。改行形式の既存設定による警告だけで、空白エラーはない。

**変更ファイル（26件）。**

- 入力／consumer: `src/key_input.rs`、`src/logger.rs`、`src/touch_correlation.rs`、
  `src/ui_fullscreen.rs`、`src/ui_main.rs`、`src/similar_preview.rs`、`src/test_script.rs`、`src/app.rs`。
- App: `src/app/detached_activation_gesture.rs`、`src/app/multiwindow_scenario_tests.rs`、
  `src/app/canvas_gesture.rs`、`src/app/sidecar_restore.rs`、`src/app/update_phases.rs`。
- vendor: `vendor/egui-winit/src/lib.rs`、`vendor/egui-winit/src/pointer_input_observer.rs`、
  `vendor/eframe/src/native_window_input.rs`、`vendor/egui/src/data/input/event.rs`、
  `vendor/egui/src/input_state/mod.rs`、`vendor/egui/src/context.rs`、
  `vendor/egui/src/context/pointer_cancellation_tests.rs`、3 crateの `PATCHES.md`。
- 文書: 本書、`docs/detached-rework-plan.md`、`docs/architecture-overview.md`。

独立実装担当とは別のSol / xhighによる完成source監査はACCEPT、追加P1 / P2なし。
テスト実行は実装担当が所有し、上記証跡を共有する。実Windowsのcapture / button tracking / focusの
通知列、part2a / part3との実測対応、実機での製品動作は未確認。
全件gate・build-dev・製品起動・commit / pushは実施しない。
見た目・レイアウトは変更せず、ui_snapshotは実行しない。

### 5.9 位置なし終端を操作の認可から分離する（2026-11-15、再レビュー対応）

基点は `1f7d9de2b`。再レビュー（Sol / xhigh）は `PointerButtonReleased` の必要性と
構造修正に合意し、保持終端をクリック／dropの権限へ転用していたconsumerを指摘した。
設計担当の規則は「位置なしの終端は保持だけを終え、操作を認可しない」。
例外は案1のParked / ParkedLiveの復帰だけであり、既存activation経路は維持する。
通常dragの終了通知は消さず、クリック／短押し／drop先／seek／close等の確定資格と分ける。

**共有の正本。** `PointerEvent::Released` に `position: Option<Pos2>` を保持する。
正常な `PointerButton` UpだけがSome、`PointerButtonReleased`はNoneで、クリック分類の
`click=None`とは別の事実である。正常dragのUpもclick=Noneになり得るので、clickの有無を
代用してはならない。`any_released` / `button_released` / `drag_stopped`は引き続き保持の終端通知。
`PointerState::button_events()` の順序付き `PointerButtonEvent` と `button_releases()` は
同じcanonical列からのread-only projectionで、解放点を後のhoverや過去のpressから作らない。
`button_release_position(button)` は当該buttonの最後のrelease位置だけを返す。
`button_click_position(button)` はclickとして受理された終端の実点だけを返す。
旧終端→新pressを同frameで扱うrecognizerは順序付きprojectionを消費し、frame全体のboolで取り消さない。

検討した簡素化は、(a)位置なし終端を `any_released=false` にする、(b)クリック判定だけを使う、
(c)frame全体の「位置なしUpあり」で一括取消にする、の3つ。
(a)は通常dragの保持を残し、(b)は正当なdrag releaseを失い、(c)は後続の新しい操作まで捨てるので採らない。
新しいApp field、時間窓、座標推定、retryは追加しない。

**P2-1／独自recognizer。** 通常viewerのpassive右押下が開始位置へfallbackし、
位置なしUpを移動量0のShortTapとしてCloseへ渡していた。viewer／grid／deferred transport、
ring・mouse gesture・middle short-clickを同じcanonical button列へ揃える。
既知Upはそのeventの点だけで確定し、未知Upは既存ownerを取消し、後続の新Downは別の保持として受け付ける。
OSのnative media入力の位置付き経路を、この判定のために変更しない。

**P2-2／D&D。** `Response::dnd_release_payload` がany_releasedと最終contains_pointerを組み合わせ、
未知Upの後に来たMoveや古いinteract_posで製本のMovePageを作っていた。
payloadは単一のDragAndDrop owner内で、現在のheldと、受理済みの位置付き終端readyを分ける。
pass開始時、既存heldを最初の終端へ一度だけ結ぶ。未知Upなら破棄、既知Upならその点とviewportを持つready。
UIが後から作った新heldには旧終端を適用せず、終了時に捨てるのは同viewportのreadyだけとする。
旧known dropと新heldは同frameに共存し得る別操作なので、排他的なHeld/Released enumに押し込めない。
ready列は当該passで実際に終わった操作の投影であり、再activation等まで持ち越す新しいApp pendingではない。
public payload/hoverはheldを優先し、dropはreadyだけを消費する。
これにより、別buttonのdownやpass番号から新旧payloadを推測する必要がない。
drop先は、通常と同じprevious-widget／layer／modalのhit-testを実release点で評価する。
最終hoverのcontains_pointerはdropの認可に用いない。child passは親Releasedの消費・退役を行わない。

`Context::get_response` のraw-release loopも、最終snapshotで既に確定したDRAGGED／IS_DOWNを
旧Upで再度OFFにしていた。levelの正本を最終memory／snapshotへ一本化し、新pressのdragを保持する。
同じwidgetに再度pressした場合も、releaseで旧dragが終わった事実から新drag_startedを出す。
最終的に新しい同widgetの保持があるResponseへ、旧drag_stoppedを付けない。
停止した旧dragの位置は、interactionでそのIdを実際に終えた終端と一緒にsnapshotへ確定し、
`Response::drag_stopped_pos()` から公開する。未知Up／Escape等の取消ではNoneにする。
frame中の最初のbutton releaseを参照するだけでは、Escape後の別clickのUpを借用するため採らない。
同frameの新しい保持がある場合、通常のinteract_posは新操作を示すため、
停止操作の確定にはこの専用projectionを使う。seekの目標解決側も同じprojectionへ揃える。
同frameに完結した次の正常clickもあり得るため、clickの目標点、現在のdrag開始／更新点、
旧dragの停止点をそれぞれの操作から得る。後着hoverや別操作の点を共有しない。
通常dragは終わるが、古いinteract_posを操作の座標として再公開しない。

**P3／page-turn。** 位置なしUpで該当buttonの `PointerPresses` deliveryだけを退役する。
navigation候補は作らず、他button・touchの所有には触れない。

**同型consumer。** 全Rust sourceの検索式、数え方、全source siteとconsumerごとの判定は
[監査表](positionless-release-consumer-audit.md)を正本の付表とする。
click Someを必要とする通常widget、保持だけのcleanup、raw ordinal transport、操作確定を区別する。
追加修正は、一覧／Collection／toolbar／detailsの並べ替え、text選択、seek track／strip、範囲コピー、
外部scroll captureのshort press／bar終端、通常scrollのrelease inertiaなど。
panelのresizeも、停止時の新しいcollapse／expandを解放位置なしで認可しない。
すでにdrag中に適用した幅はresize widget所有のgeometryとして保存し、終端で保持だけを終える。
適用済geometryの保存を、未知releaseの位置に転用しない。
既存PanelStateは閉じて再展開するためのpre-drag自然幅で、最後のlive幅ではない。
前widgetのrectもanimationでshift済みなので、そこから自然幅を復元する案は採らず、
実dragで適用した幅1値だけを同じresize IDへ持たせる。new start／stop／idle／再出現で寿命を閉じる。
すでに適用した連続編集値の保存やslider／panのowner清掃を、新しいdrop先の確定と混同しない。
egui-winitのsimulate_touch_screenも、未知mouse Upから最後の検出点を持つTouch Endを生成していたため、
Touch Cancelで接点だけを終える。physical mouseの終端とParked復帰の例外は保持する。

**赤確認。** 製品未変更時の焦点は **5実行／1成功／4失敗、exit 100**
（`target/rm55-r2-red-main.log`）。P2-1・P3・middle短押し・右flick/gestureを再現した。
実Stateからの製本D&Dは **4実行／0成功／4失敗、exit 100**
（`target/rm55-r2-red-dnd.log`）。後着Move、古いinteract_pos、既知／未知Up混在、新drag寿命を検査する。
追加の赤filterは共有runnerが別作業のfull gate待機中だったため、無実行のまま取り下げて緑検証へ回した。
この待機を製品のテスト失敗として数えない。

中間の本体焦点は **1,011実行／1,002成功／9失敗、exit 100**。
追加fixtureが新Downと同じframeで即drag_startedを要求した点、settling後のframeで一度限りの
drag_stoppedを検査した点、連続repaintする保持中にrunで収束を要求した点、canonical寸法未導入を修正した。
pendingのcaptureと次frameのdrag、実終端の1回を分けて検査し、drop先・項目identity・非操作の期待は維持する。
その過程で、Book／Collectionのtileがindex依存auto IDのため、旧dropによる並べ替え後に
新pending pressの所有が別項目へ移る経路も確認した。配置とauto IDの進行は従来どおりのまま、
interaction IDをBookのentry key／Collectionのentry IDへ固定する。
並べ替え前後で同じBのID・capture・選択・anchor・新dragを実handlerで検査する。
コンパイル中間失敗は条件式の括弧、共有cleanupメソッドの可視性、テストのroot_ui引数の3種類を修正した。
最終焦点の最初のcompileも追加text fixtureのE0502で **0件実行、exit 101**
（`target/rm55-r2-focused-final-compile.log`）。DerefするテストAppの世代番号をlocalへ先に読み、
cacheのmutable借用と分離した。製品処理の変更はない。
続く焦点は **1,015実行／1,009成功／6失敗、exit 100**
（`target/rm55-r2-focused-final-r1-fail.log`）。6件は追加seek fixtureの観測方法によるもの。
`egui_kittest 0.37 Harness::step` はqueueの各eventごとに `step_impl` を実行する
（registryの `egui_kittest-0.37.0/src/lib.rs:368–378`）。実Stateから得た1 batchを
`harness.event` の列へ渡したため、終端後のMovedが別frameとなりlast観測のedgeが消えていた。
State生成Vecを `input_mut().events` へ一括導入し、一度のstepで実handlerへ渡す形へ修正した。
手作りreleaseや期待値への置換はせず、終了・非操作・次click／dragの点の全assertを維持した。
このfixture修正も別contextのSol / xhighがread-onlyで照合し、正規の同batchを保つ修正としてACCEPT。
最小6件の再実行は **6実行／6成功／0失敗、exit 0**。
次のFは **1,015実行／1,014成功／1失敗、exit 100**
（`target/rm55-r2-focused-final-archive-fail.log`）。失敗は既存の
`section1675_inner_resume_fence_across_composition_wait` の `sidecar_restore_active` 事前条件で、
今回の新規回帰はすべて成功した。同一sourceの単独確認は **1実行／1成功、exit 0**。
この失敗を解消した製品変更は入れず、非同期fixtureの条件を別途照合した。
同一sourceで後続Fは **1,015/1,015成功**、Rは **2,687/2,687成功**（4 slow）となった。
sourceで確認できたのは、`book_resume_db.rs:807` のPauseがadmitted sequenceを増やさず、
`:859` のadmitted/completed一致時にはpublished snapshotから即replyする経路である。
`sidecar_restore.rs:2231` でindex完了後Resumingへ進み、`:2673`／`:2728` で同poll中に
依存が完了するとsidecarを保持せずcomposition待ちへ移れる。該当fixtureとこれらの製品処理は今回差分なしで、
native／UI入力の経路も通らない。saved=false・index完了済みの時期依存が有力だが、
実失敗のlive/saved組合せはログにないため確定できない。キャッシュ済み即時完了を遅らせる変更も行わない。
vendorの中間 **72実行／70成功／2失敗**、次回 **72実行／71成功／1失敗** は、
旧releaseが新しい同widgetの保持をclearする旧挙動を期待したfixtureを修正したもの。
新holdの通常位置と旧drag_stopped_posを同時に検査した後、**72/72成功、exit 0**。

追加panel回帰の中間は **83実行／80成功／3失敗、exit 100** が2回。
空contentsではPanelStateがdefault幅ではなく自然な最小幅を保存することと、最初のsizing passでは
handleがまだ入力対象でないことを、fixtureの事前条件へ反映した。
実handleへのpress、live幅122／pre-drag保存幅150、既適用cache、unknown／known終端後の幅と
pending capture、取消／非表示後のcache退役をすべて維持して最終83件が成功した。

**最終検証。** Cargoはすべて指定runner経由で直列。
共有lock/TEMPへの書込不可の警告時は、runner既定の低優先度・6並列で続行した。
実装者とは別contextのGPT-6.1 Sol / xhighによる最終source reviewは **ACCEPT、要修正事項なし**。
位置資格、旧ready／新held、seekの3種の点、通常drag終端、後続入力、owner別清掃を照合し、
症状パッチではなく構造修正であると確認した。レビュー担当は編集・Cargoを実行せず、以下の実行はroot担当。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner "codex/767-rm55" -WorkDir G:\home\mimageviewer-767s2 -Command "<下記のCargoコマンド>"
```

F（本体焦点）:

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/realmachine55_|realmachine50_|native_unknown_|native_positioned_|detached_activation|non_client|pointer_input_observer|viewer_chrome|right_drag|page_turn|composition|grid_reclick_open/)' --no-fail-fast --test-threads 6
```

S（失敗したseek batchの最小再確認、Fより先に実行）:

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/realmachine55_.*seek/)' --no-fail-fast --test-threads 6
```

A（既存archive fence失敗の単独照合）:

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/section1675_inner_resume_fence_across_composition_wait/)' --no-fail-fast --test-threads 6
```

R（本体回帰、今回の共有consumerと従来の入力対照）:

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/realmachine50_|realmachine55_|detached_activation|native_detached|non_client|pointer_input_observer|parked|native_video|touch|key_input|native_focus|grid|composition|page_turn|viewer_chrome|right_drag|ui_text|still_seek|capture_region|canvas_gesture|similar_preview|sidecar_restore|navigator|overflow|logger/)' --no-fail-fast --test-threads 6
```

E / W / G / L（vendor）:

```powershell
cargo nextest run --manifest-path vendor/egui/Cargo.toml --lib -E 'test(/release|unknown_|pointer_cancellation|input_state|external|slider|interaction|old_drag_stop|panel/)' --no-fail-fast --test-threads 6
cargo nextest run --manifest-path vendor/egui-winit/Cargo.toml --lib --no-fail-fast --test-threads 6
cargo nextest run --manifest-path vendor/eframe/Cargo.toml --no-default-features --features default_fonts,wgpu --lib -E 'test(/realmachine55_|viewport_retirement|native_window_input/)' --no-fail-fast --test-threads 6
cargo nextest run --manifest-path vendor/eframe/Cargo.toml --no-default-features --features default_fonts,glow --lib -E 'test(/realmachine55_|viewport_retirement|native_window_input/)' --no-fail-fast --test-threads 6
```

| コマンド | 実行／成功／失敗 | exit code | 証跡 |
| --- | --- | --- | --- |
| S | 6／6／0 | 0 | `target/rm55-r2-seek-batch.log` |
| A | 1／1／0 | 0 | `target/rm55-r2-archive-fence.log` |
| F | 1,015／1,015／0 | 0 | `target/rm55-r2-focused-final.log` |
| R | 2,687／2,687／0 | 0 | `target/rm55-r2-regression.log` |
| E | 83／83／0 | 0 | `target/rm55-r2-vendor-egui-gate.log` |
| W | 73／73／0 | 0 | `target/rm55-r2-vendor-winit-gate.log` |
| G | 21／21／0 | 0 | `target/rm55-r2-vendor-wgpu.log` |
| L | 21／21／0 | 0 | `target/rm55-r2-vendor-glow.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | compile成功 | 0 | `target/rm55-r2-check.log` |
| `cargo fmt --manifest-path vendor/egui/Cargo.toml` | 整形 | 0 | `target/rm55-r2-vendor-format-egui.log` |
| `cargo fmt --manifest-path vendor/egui-winit/Cargo.toml` | 整形 | 0 | `target/rm55-r2-vendor-format-winit.log` |
| `cargo fmt`（最後のCargo操作、引数なし） | 整形成功 | 0 | `target/rm55-r2-fmt.log` |
| `python scripts/check_ui_glyphs.py` | 危険glyph 0 | 0 | 15:46実行 |
| `git diff --check` | whitespace異常0 | 0 | 最終文書更新後 |

最終sourceの監査は **1,613使用箇所／546関数＋2 macro**、直接認可を修正したconsumerは **25関数**。
共有のcanonical／snapshot基盤と、そのprojection経由だけで安全になるcallerは直接修正数へ重複加算しない。
全使用箇所・関数別の判定は監査付表へ記録し、未分類は0。

変更ファイルは **29件**（製品・共有vendor・回帰・文書）:

- `docs/architecture-overview.md`
- `docs/depth-parallax-realmachine50-investigation.md`
- `docs/detached-rework-plan.md`
- `docs/positionless-release-consumer-audit.md`
- `src/app.rs`
- `src/app/gamepad_input.rs`
- `src/app/tests.rs`
- `src/ui_composition_editor.rs`
- `src/ui_dialogs/collections.rs`
- `src/ui_fullscreen.rs`
- `src/ui_fullscreen/page_turn.rs`
- `src/ui_fullscreen/page_turn/input_regression_tests.rs`
- `src/ui_fullscreen/viewer_chrome.rs`
- `src/ui_fullscreen/viewer_chrome/release_tests.rs`
- `src/ui_main.rs`
- `src/ui_text.rs`
- `vendor/egui-winit/PATCHES.md`
- `vendor/egui-winit/src/lib.rs`
- `vendor/egui-winit/src/pointer_input_observer.rs`
- `vendor/egui/PATCHES.md`
- `vendor/egui/src/containers/panel.rs`
- `vendor/egui/src/containers/scroll_area.rs`
- `vendor/egui/src/context.rs`
- `vendor/egui/src/context/pointer_cancellation_tests.rs`
- `vendor/egui/src/drag_and_drop.rs`
- `vendor/egui/src/input_state/mod.rs`
- `vendor/egui/src/interaction.rs`
- `vendor/egui/src/lib.rs`
- `vendor/egui/src/response.rs`

未確認はWindowsの実capture／OS通知／実UIでの操作結果と、既存archive失敗の実iteration。
実Stateとegui widgetのheadless回帰を
実機確認の代わりとは扱わない。描画・layout・UI文字列の変更はなく、ui_snapshotは実施しない。
全件gate・build-dev・製品起動・commit／pushは指示に従い実施しない。

### 5.10 依頼56: 最大化／復元ボタンの取消理由と実OS順序の採取（2026-11-15）

基点はHEAD `3f35452ef`（製品 `81406ab92`）。`git log -3` とclean treeを確認した。
**原因は未確定であり、今回、認識規則・phase遷移・finish条件を変更しない。**
理由付き診断と、同じ製品実装を使う独立Win32 probeを追加して次の実機採取へ渡す。
BA-7（操作の所有と確定点）の確認であり、待機・再試行・時間窓・rectによるHWND推定・App pendingは追加しない。

#### 観測と確定できる範囲

依頼時は結果未着だったが、作業中に `C:/miv-subpc/results-56-767/notes.md` とlogsが到着し照合した。
観測者はすべてサブPC担当。part1の399.720／449.983／460.359sではParkedLiveのwindow8、
part3の215.394／243.527／254.718／269.121／279.313／288.589sではParkedのwindow1について、
OSの最大化／復元は行われるが `normalized NonClientClick GestureCancelled` だけが残る。
依頼55の通常最大化173.365sはcommit。依頼56のcaption click／drag／double clickは成功し、
button外releaseはOS動作も再開もなく、×はCloseOwnedだった。ERROR／WARN／panic／HWND conflictは0件。
報告中の「戻ってから離す」は画面操作ツールの制約により未実施であり、成功確認へ含めない。

既存ログにはraw message列とreasonがないため、上の事実だけで先行取消の原因は決められない。
`GestureCancelled` はModalOwned／RetiredRegistrationと別decisionなので、target更新に伴う
その2理由を当該行の直接原因とは扱わない。NC開始時の別操作のCancelModeは、その元操作の種類で配送される。
同じNC操作を理由なしにtarget更新でCancelModeへ替える経路は確認できなかった。

#### 製品の診断

`DetachedActivationRequest::native_outcome_diagnostic` をNonClient／NativeMediaTouchの共通sinkで呼び、
既存decision mappingを変えず、通常の `logger::log` へ1行を渡す。
取消は `reason=CaptureLost`／`FocusLost`／`CancelMode`／`CaptionButtonOutside`等、
Completed／CloseOwnedは `reason=None`。window／registration／context／expected_stateは従来どおり。
判定・破棄のイベント境界だけで出し、毎frameのログは追加しない。

```text
[detached-activation] boundary=normalized source=NonClientClick decision=GestureCancelled reason=CaptureLost window=... registration=... context=... expected_state=Parked
```

本体loggerへ直接渡すため、dependencyのInfo targetを開ける必要はない。
NativeMediaTouchも同じformatterを通り、sourceは既存どおりNativeMediaとする。

#### 通知列ごとの現行結果（ソース上の判定表、実機列の断定ではない）

以下は通常mouseでHTMAXBUTTONを押した場合。D＝subclassのNC Down、U＝所有Up、
C＝一致するSC_MAXIMIZE／SC_RESTOREのdispatch、F(D)＝外側DefSubclassProc(D)の返却。
hookはPM_REMOVEだけを観測し、自身は開始producerにならない。

| 通知列／境界 | 現行結果 | 決定する事実 |
| --- | --- | --- |
| D → 内側U → C → F(D) | Completed | Released → ButtonActionCommitted |
| D → C → capture／focus／CancelMode → F(D) | Completed | 確定後の通常取消はphaseを巻き戻さない |
| D → 内側U → capture解放 → C → F(D) | Completed | CaptureLostの対象はPressedだけ |
| D → CaptureChanged → U／C → F(D) | Cancelled(CaptureLost) | buttonにはcaptionのlParam=0例外がない。先行取消からCで復活しない |
| D → KillFocus → U／C → F(D) | Cancelled(FocusLost) | C前のPressed／Releasedで取消 |
| D → CancelMode → U／C → F(D) | Cancelled(CancelMode) | 同上 |
| D → Escape → U／C → F(D) | Cancelled(Escape) | 同上 |
| D → 異なるhitのU → C → F(D) | Cancelled(CaptionButtonOutside) | releaseのhitが押したbuttonと一致しない |
| D → Uの位置変換失敗 → C → F(D) | Cancelled(CancelMode) | ClientToScreen失敗。既存failure通知も出る |
| D → 内側U → Cはhookだけ／不一致command → KillFocus → F(D) | Cancelled(FocusLost) | dispatch中の一致commandでなければ確定しない |
| D → F(D) → C（Pressed／AwaitingRelease、Uなし）→ CaptureChanged | Cancelled(CaptureLost) | そのscopeのPressedは一致SCだけでは確定せず、後のcapture取消が残る |
| raw NC pointerでC前にpointer cancel／capture changed | Cancelled(PointerCancelled) | pointer ID一致が必要。普通mouseの直接理由ではない |
| D → C → target block／retire | Cancelled(ModalOwned／RetiredRegistration) | bindingの取消は確定後も有効。ログdecisionはGestureCancelledではない |
| D → F(D)、U／Cなし | 未完了・AwaitingRelease | buttonはcaptionと異なりDown returnだけでは完了しない |
| HTCLOSEの操作 | CloseOwned | 再開へ変換しない |

**依頼57の結論:** 実測は「異なるhitのU → 一致C」の列だった。release hitの引き直しを撤去し、同じ押下call内の一致Cと押下返却へ一本化する（§5.11）。上表は依頼56時点の履歴として残す。

参照関数は `key_input.rs` の `native_non_client_location`／`native_non_client_button_location`、
`native_detached_message_with_hit_test`、`finish_native_detached_dispatch`、`set_native_detached_input_targets`。
`e9c909f56` と `81406ab92` を比較し、依頼55の `25f3f9e41` も照合した。
旧版にもPressed時CaptureLost、KillFocus／CancelMode、release座標変換＋NCHITTESTは存在する。
新側はmotionでの取消と8px距離判定を撤去し、一致SCの確定を足す方向で、
同一message列・同一ownerから「旧Completed、新Cancelled」となる単一の新条件は見つからなかった。
`PointerButtonReleased` はegui client経路であり、このNC reducerへ直接入力しない。
到着した全logsで `ClientToScreen`／`non-client button` のfailure記録は見つからなかった。
変換失敗の経路には独立した通常logがあるため、単にWARNが0という事実より強い反証となるが、
実際のcancel理由は新reasonで確かめる。

[MicrosoftのWM_CAPTURECHANGED仕様](https://learn.microsoft.com/en-us/windows/win32/inputdev/wm-capturechanged)は、
自身のReleaseCaptureでも通知を受けることを規定する。
[WM_LBUTTONUP仕様](https://learn.microsoft.com/en-us/windows/win32/inputdev/wm-lbuttonup)の座標はclient相対で、
現行のClientToScreenはその契約に従う。
[ReactOSのThemeHandleButton](https://github.com/reactos/reactos/blob/master/dll/win32/uxtheme/nonclient.c#L663)は、
GetMessageでUpを内部消費し、ReleaseCapture後に一致SCをSendMessageする。
これはWindows 11の実列の証拠ではなく、hookがUpを観測できれば現行でCompletedになる列の参考である。
「ReleaseCaptureがSCより先だから今回だけ失敗」とは断定しない。

#### 独立probeの共有境界と寿命

`src/bin/native_detached_probe.rs` を `dev-tools` 必須の補助binとして登録し、通常配布のgraphへ含めない。
自身で標準枠付きtop-level窓を1つ作り、App／renderer／backend／設定／本体loggerを初期化しない。
headless egui Contextはlogical registrationだけを所有する。
`key_input::native_detached_probe::ProbeSession` が実HWNDへ製品の `install_window_input` と
`NativeKeyInputLease` を接続し、Parked相当のtargetを登録する。reducer／subclass／hit testの写しはない。
UIthreadのWH_GETMESSAGEはHC_ACTION＋PM_REMOVEだけを製品の `observe_native_detached_message` へ渡す。
実subclassは従来どおりDefSubclassProcを呼び、製品のdispatch／finishを通す。
probeのoutcomeは記録だけに使い、targetをActiveへ変更しないので続けて複数操作を比較できる。

dev-tools限定traceはdispatch／hookの前後binding、実release_hit、native finish前後を出す。
scopeはinput mutexより前に作り、後にdropするのでsnapshot／sink呼出しはowner lock外。
sinkのファイルI/Oは専用writerへ配送し、stdoutとファイルに同じ順序で1行ずつ保存する。
製品の通常featureではこのtraceはコンパイルされない。
scope unwind時はlease退役 → hook解放 → 自窓破棄、すべてのtrace sender解放後にwriterをjoinする。
×等で先にWM_NCDESTROYとなった場合も、その既存terminal処理とleaseの明示退役を重複実行しない。
finishの引数は元関数が保持しないため `parameters=not-captured` とし、架空のwParam／lParam=0を記録しない。

#### PowerShellでのビルドとサブPCでの使用

ビルドは本体の配布buildとは別。エージェントはビルドとheadless testだけを実行し、probeも起動しない。

```powershell
# リポジトリroot（実際の検証では指定run-heavy.ps1経由）
cargo build -p mimageviewer --features dev-tools --bin native_detached_probe
# 出力: G:\home\mimageviewer-767s2\target\debug\native_detached_probe.exe
```

コピー用の専用フォルダは `G:/home/mimageviewer-767s2/target/rm56-native-detached-probe/`。
**このフォルダ全体**をサブPCの `C:/miv-test-v5/native-input-probe/` へコピーする。
内部exeは `native_detached_probe.exe`（33,454,080 bytes、約33.5 MB）。実装共有によりPE importに
FFmpegの `avcodec-61.dll`／`avfilter-10.dll`／`avformat-61.dll`／`avutil-59.dll`／
`swresample-5.dll`／`swscale-8.dll` が残るため、同じvendorの6 DLLを隣へ配置した。
実行ファイル＋6 DLLは計151,267,344 bytes。`FFmpeg-LICENSE.txt`、README、PE-IMPORTS.jsonも同梱する。
DLLの推移依存は同梱DLLまたはWindowsのsystem DLL／API setに解決することを静的に照合した。
この静的確認を起動確認とは扱わない。製品の配布ディレクトリへは追加していない。

```powershell
Set-Location C:\miv-test-v5\native-input-probe
# manualが既定。自分の窓で最大化→復元、caption click／drag／double click、
# buttonの外でrelease／外へ出て戻ってreleaseを行い、最後にEscか×で終了。
.\native_detached_probe.exe --log .\manual.log
$LASTEXITCODE

# autoは明示したときだけ。自窓の最大化→復元を各1clickし、終了する。
.\native_detached_probe.exe --auto --log .\auto.log
$LASTEXITCODE
```

logは起動ごとに上書きするため、比較するrunには別名を指定する。

autoはOSのTITLEBARINFOEXのbutton矩形から点を取り、WM_NCHITTEST=HTMAXBUTTON、
WindowFromPoint=自HWNDを送信直前に確認する。異なる窓・不明な矩形・不一致hitなら入力せずerrorで終了する。
1段階につきSendInputはmove／down／upの1 batchだけ。WM_SIZEの最大化／復元完了を受け、
外側DispatchMessageが返った後に次の段階へ進む。modal loop中へ次のclickを入れない。
待機timer／retryはなく、OSの完了通知が来なければEsc／×で中断する（auto未完了はexit 1）。
手動時は入力を合成しない。ツールが書くのは指定logだけで、mIVの通常profileには触れない。

#### 出力の読み方と次の判断

- `t_us` はtool開始からの時刻。`source=hook_removed` は除去済みMSGのtimeも持つ。
- `source=wndproc` は自窓のmessage／wParam／lParam。NCのhitはpayload、NCHITTESTは実返値を記録する。
  client messageにはhit payloadがないためNoneとし、HTCLIENTを推測しない。
- `source=shared boundary=hook/dispatch.before/after` で同じHWNDのbinding、Pressed／Released／Committed／Cancelledとscopeを追う。
- `boundary=release_hit` で実Upのhit照合を確認する。`hit=None` は変換失敗であり位置の推定は行わない。
- `boundary=finish` は各messageのDefSubclassProc返却後。特に外側NC Downのfinishと、
  内側のUp／SC等のfinishをmessage番号で区別し、`boundary=outcome` の最終Completed／Cancelledとreasonを読む。
- 先行取消、内側Uの有無、SCのdispatch/hook別、hitの変化を上表と照合する。
  probeで再現しなければ、それ自体をmIVの改善証明にせず、製品の新reasonとhost差を比較する。

**最終検証記録。** 指定run-heavy、Owner `codex/767-rm56` で直列。

```powershell
cargo build -p mimageviewer --features dev-tools --bin native_detached_probe
cargo test -p mimageviewer --features dev-tools --bin native_detached_probe -- --test-threads 6
cargo nextest run -p mimageviewer --lib -E 'test(/native_detached_normalized_diagnostic|native_detached_caption|native_detached_raw_caption|non_client|realmachine55_parked_live_native_non_client/)' --no-fail-fast --test-threads 6
cargo nextest run -p mimageviewer --lib -E 'test(/native_detached|non_client|caption|detached_activation|realmachine55_|parked|native_video/)' --no-fail-fast --test-threads 6
cargo check -p mimageviewer --bin mimageviewer-core
cargo fmt
```

| 検証 | 件数／exit code | 証跡 |
| --- | --- | --- |
| probe build | 1 exe、exit 0 | `target/rm56-probe-build.log` |
| probe pure unit | 3実行／3成功／0失敗、exit 0 | `target/rm56-probe-unit.log` |
| 本体焦点 | 6実行／6成功／0失敗、exit 0 | `target/rm56-focused.log` |
| 本体回帰 | 663実行／663成功／0失敗、exit 0 | `target/rm56-regression.log` |
| core check | 1 core対象、exit 0 | `target/rm56-check.log` |
| 最後の引数なしfmt | exit 0 | `target/rm56-fmt.log` |
| `python scripts/check_ui_glyphs.py` | 危険glyph 0、exit 0 | 最終sourceで実行 |
| `git diff --check` | whitespace異常0、exit 0 | 最終文書更新後 |

PE import／exeサイズ・SHA256は `target/rm56-pe-imports.json` に採取した。
exe SHA256: `5769572b5d2caa20f348925d97e237f3a164d7652e89f92b1ae287bab9da1ba5`。
共有full lock（他作業の配布build）が解放されるまで待機し、開始後はlock領域／G: TEMPへの
書込不可の警告に従い低優先度・6並列で実行した。他作業のprocess／lockは変更していない。
変更ファイルは `Cargo.toml`、`src/app/detached_activation.rs`、`src/key_input.rs`、
`src/key_input/native_detached_probe.rs`、`src/bin/native_detached_probe.rs`、本書、`docs/detached-rework-plan.md` の7件。
独立source review（Sol / xhigh）は、認識規則を変えない診断と同じownerを使うprobeを
症状パッチではない構造としてACCEPT。実API compileと実機順序の確認をその判定へ代用しない。
未確認は実OS列、native theme／mIV host差、手動・auto実行、実機reason。製品起動・全件gate・build-dev・commit／pushは実施しない。

### 5.11 依頼57: caption buttonの確定をOSの一致コマンドへ統一（2026-11-15）

基点は `5919fe4dc`。観測者はすべてサブPC担当で、根拠は
`C:/miv-subpc/results-57/notes.md`、`manual.log`、`auto.log` と製品logs。
manual sequence 1～6は、HTMAXBUTTONのDown → hookのclient Up →
CaptureChanged(0) → subclassのSC_MAXIMIZE／SC_RESTORE → SCのfinish → Downのfinish。
Upのclient座標を画面座標へ戻して同じHWNDへWM_NCHITTESTを送っても、返値は
最大化時HTCAPTION、復元時HTMINBUTTONとなり、OSの実行前にCaptionButtonOutsideへ変わっていた。
製品も6回とも同理由で取消となった。sequence 8の外で離す操作にはSCが無く、OSも最大化しない。
sequence 7のcaption clickはCompleted。表示倍率125%／snap一覧の有無は観測条件であり、
倍率が原因だとは断定しない。依頼55で1回commitした理由は未解明のまま残す。

**確定点と終端。** HTMINBUTTON／HTMAXBUTTON／HTHELPは、同じ押下のnative call中に
subclassでdispatchされた一致WM_SYSCOMMANDだけを選択の事実とする。
開始を所有した押下のDefSubclassProc返却（後述のcall tokenで照合）だけで終端し、一致SCの結果（最小化は後述のMinimizeOwned）を返し、無ければ
Cancelled(CaptionButtonOutside)とする。このreasonは「OSがボタンを選択しなかった」の意味になる。
Upの位置・有無、CaptureLost／FocusLost／CancelMode／Escapeはボタンの採否に使わない。
targetのblock／retireは既存bindingの取消として維持し、後続SCで復活させない。
HTCLOSEのCloseOwned、caption／resize、ClientBoundaryは変更しない。
CLAUDE.md「設計の簡素化」の「確定の時点を1つにする」「OSと同じ経路に揃える」を採り、
位置照合とSCという二重決定、release待ちとの組合せを取り除く（BA-7）。

**撤去範囲。** ボタン用release位置照合、ClientToScreen／WM_NCHITTEST送信、専用の座標helper、
hit-test注入用のproduction seamとprobeのrelease_hit traceを撤去する。
ボタンはReleasedにもAwaitingReleaseにも移らない。これらの共用enum variantは
caption／resizeなどの非ボタン操作が使うため残す。

**初回のtouch／penの仮定（後述のタッチ実測で置換）。** WM_NCPOINTERDOWNも同じ一致SCだけを採用し、その押下call返却で完結させる。
押下返却後のUpやSCを待つ状態は作らない。raw pointerがOSの追跡を同期実行するのか、
昇格mouseで実行するのかは実機未確認。昇格mouseを別ownerにしない既存規則は維持する。

**probe。** §5.10のautoの事前WM_NCHITTEST確認は本節で置き換える。
初回実装ではTITLEBARINFOEXの最大化ボタン矩形と送信直前のWindowFromPoint=自窓だけを確認して入力し、
実際のWM_NCLBUTTONDOWNのwParamがHTMAXBUTTONであることを観測した後だけWM_SIZEを採用する。
不一致なら失敗終了し、追加clickは送らない。最大化→復元を各1回、手動／autoの起動方法は§5.10と同じ。
コピー用 `target/rm56-native-detached-probe/` はexeだけ更新し、DLLは変更しない。
現在のtraceにはrelease_hitは無く、SC dispatchと押下finish、outcomeを読む。
送信条件は下記独立レビューP2-4の追加検査を含む。

通知列の新しい判定は次のとおり。hookだけのSCは確定の事実にしない。

| 同じ押下call内の列 | 押下返却時の結果 |
| --- | --- |
| MAX Down → client Up（再照会ならHTCAPTION）→ CaptureChanged(0) → dispatch SC_MAXIMIZE | Completed |
| MAX Down → client Up（再照会ならHTMINBUTTON）→ dispatch SC_RESTORE | Completed |
| Down → Up → CaptureChanged、SCなし | Cancelled(CaptionButtonOutside) |
| Down → KillFocus／CancelMode／Escape／capture取消 → 一致SC | 選択SCの結果（MIN＋SC_MINIMIZEはMinimizeOwned、他はCompleted） |
| MAX Down → 不一致SC／hookだけの一致SC | Cancelled(CaptionButtonOutside) |
| MIN Down → dispatch SC_MINIMIZE | 初回はCompleted、依頼58の決定でMinimizeOwnedへ変更（後述） |
| HELP Down → dispatch SC_CONTEXTHELP | Completed |
| Down → target block／retire → 一致SC | ModalOwned／RetiredRegistration（復活しない） |
| Down返却（SCなし）→ 遅着SC／Up | 返却時に取消済み、追加の結果なし |
| HTCLOSE → SC_CLOSE | CloseOwned |

**回帰と仕様変更の対応。** 実測列の赤テストは旧実装で1件実行・1件失敗、nextest exit 100
（run-heavy外側PowerShellのexit 1）。実出力は
`left: [NonClient(Cancelled(CaptionButtonOutside))] / right: [NonClient(Completed)]`、
証跡 `target/rm57-red.log`。最終テストは表示しないmessage-only HWNDのWndProcに
HTCAPTION／HTMINBUTTONを返すfakeを設置し、実SendMessageでfakeの動作を確認してから
query回数を0に戻す。製品reducerの実測列を通した後も0であることを検査する。
旧production注入seamをこの検査のために残さない。

意味を更新したテストは8件（prefixの省略なしで列挙）。通常のclient／caption／resizeの回帰は維持する。

| 旧テスト名 | 変更後の検査 |
| --- | --- |
| `realmachine55_caption_button_native_command_cannot_revive_true_cancellation` | `realmachine57_caption_button_native_side_effects_do_not_override_os_choice`。nativeのfocus／capture／CancelMode／Escape後も一致SCを採用 |
| `realmachine55_caption_button_captured_mouse_up_uses_release_hit_until_native_return` | `realmachine57_caption_button_os_command_survives_misleading_release_hit`へ置換。実query境界のfakeが未呼出で一致SCを採用 |
| `realmachine55_caption_button_raw_committed_action_waits_for_own_up_return` | `realmachine57_caption_button_raw_committed_action_finishes_at_down_return`。pointerも押下call返却で完了 |
| `realmachine55_caption_button_unmatched_or_queued_command_is_not_execution_proof` | 不一致／hookだけのSCはFocusLostではなくOS未選択のCaptionButtonOutside |
| `native_detached_cancel_ends_at_native_return_and_does_not_resume_on_late_up` | buttonのcapture(0)ケースは押下返却時のOS未選択取消。残るcaptionの取消理由と遅着Up無効は維持 |
| `native_detached_caption_button_out_and_back_completes_with_inside_up` | `native_detached_caption_button_out_and_back_completes_with_os_command`。戻った位置ではなく一致SCで確定 |
| `native_detached_caption_button_far_motion_completes_inside_same_target` | `native_detached_caption_button_far_motion_completes_with_os_command`。最小化SCで確定 |
| `native_detached_raw_caption_button_tracks_only_own_pointer_target` | pointerの早い押下返却もSCがなければ取消し、後のUp／SCで復活しない |

block／retireの兄弟fixtureとParkedLiveの実handlerは、成功側の入力列へ一致SCを追加し、
所有context・兄弟保全・一度だけのcommitという既存の期待を維持した。
独立source review（Sol / xhigh）はBA-7の構造修正としてACCEPT、阻害指摘なし。
製品・probe・autoはエージェントから起動しない。

**検証。** すべて指定run-heavy、Owner `codex/767-rm57`、6並列、gate同士は直列。

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/realmachine57_|caption_button|native_detached_cancel_ends|realmachine55_parked_live_caption/)' --no-fail-fast --test-threads 6
cargo nextest run -p mimageviewer --lib -E 'test(/native_detached|non_client|caption|detached_activation|realmachine55_|realmachine57_|parked|native_video/)' --no-fail-fast --test-threads 6
cargo test -p mimageviewer --features dev-tools --bin native_detached_probe -- --test-threads 6
cargo build -p mimageviewer --features dev-tools --bin native_detached_probe
cargo check -p mimageviewer --bin mimageviewer-core
cargo fmt
```

| 検証 | 件数／exit code | 証跡 |
| --- | --- | --- |
| 旧コード＋実測列の赤テスト | 1実行／1失敗、nextest exit 100 | `target/rm57-red.log` |
| 最終焦点 | 14実行／14成功／0失敗、exit 0 | `target/rm57-focused-final.log` |
| 指定回帰＋新テスト | 664実行／664成功／0失敗、exit 0 | `target/rm57-regression.log` |
| probe pure unit | 5実行／5成功／0失敗、exit 0 | `target/rm57-probe-unit.log` |
| probe build | 1 exe、exit 0 | `target/rm57-probe-build.log` |
| core check | 1 core対象、exit 0 | `target/rm57-check.log` |
| 最後の引数なしfmt | exit 0 | `target/rm57-fmt.log` |

赤テストのコマンドは `cargo nextest run -p mimageviewer --lib -E 'test(/realmachine57_/ )' --no-fail-fast --test-threads 6`。
初回の緑焦点も14成功（`target/rm57-focused.log`）。その後SC_CLOSEの不一致とpointer captureの
対照を強化した最終sourceで、上記の焦点と回帰を実行した。
run-heavyはlock領域への書込不可の警告に従い低優先度・6並列で続行し、重いgateを重複実行しない。

コピー済みexeは `target/rm56-native-detached-probe/native_detached_probe.exe`、33,449,984 bytes。
SHA256は `f370650399ea78e72f56e6b7254f2666c4abafc3bf8556184d843d3c75d1fc71`。
build出力とのhash一致と6 DLLの旧manifest hash一致を検査した。exe＋6 DLLは151,263,248 bytes。
`PE-IMPORTS.json`とREADMEを更新し、DLL本体は変更していない。静的PE記録は
`target/rm57-pe-imports.json`。起動検査の代用とはしない。

変更ファイルは `src/key_input.rs`（ownerと回帰）、`src/key_input/native_detached_probe.rs`（不要trace撤去）、
`src/bin/native_detached_probe.rs`（autoとpure unit）、`src/app/detached_activation.rs`（ParkedLiveのtest入力列）、
本書と `docs/detached-rework-plan.md` の6件。vendor、App field、通常配布物は変更しない。

未確認は実OSでの修正後のmouse／pointer列と、pointerからmouseへの昇格、実機での再開。
依頼55で1回成功した条件の違いは未解明。全件gate／build-dev／製品起動／probe起動／commit／pushは行わない。

#### 依頼57の独立レビュー指摘への対応（2026-11-15）

基点は `e81ac0d94`。別会話の独立レビュー（Sol / xhigh）はREJECT、OSの確定点へ揃える
方針には構造的修正として合意した。前節の実装時source reviewのACCEPTは履歴であり、
この独立レビューの不合格を置き換えない。設計担当の決定に従いP2-1／P2-4を修正し、
P2-2／P2-3は下記の未確認・既知の残りとして扱う。

**P2-1: callの所有。** message番号だけでは同じHWNDの再入したDown／Upの返却を区別できなかった。
buttonだけでなくcaption／resizeのDown返却、raw pointerのAwaitingRelease後のUpにも
同じ欠陥を確認した。subclassがinvocationごとに `NativeDetachedCall`（一意sequence、message、
wParam）を生成し、既存scopeの `InNativeCall(call)` が開始したcallを所有する。
finishへ同じtokenを渡し、完全一致する返却だけを採用する。pointer DownではIDも照合する。
製品側のstack／App field／時間窓は追加しない。一意番号は既存のsequence発行元を使う。

非buttonでDown返却後にUpを待つ場合は、同sourceの実dispatchが `InReleaseCall(call)` を所有する。
hookが既にReleasedを記録していてもclaimし、入れ子のUpはscopeを上書きしない。
WM_EXITSIZEMOVEも同じ規則とする。Down内で観測したUpは元Downのscopeを維持する。
取消済み／CloseOwnedのAwaitingReleaseにおける既存の終端は維持する。
tokenは「どのnative callの返却か」を証明し、一致SCの採用期間は元Downの動的call期間とする。
別の物理操作のSCの由来まで新たに推測する仕組みは作らない。

**P2-4: autoの送信前条件。** TITLEBARINFOEXの矩形／WindowFromPointに加え、送信直前に
GetForegroundWindow=自窓、その前面threadのGetGUIThreadInfo成功かつcaptureが空または自窓、
Left／Right／Middle／X1／X2のGetAsyncKeyStateのhigh bitがすべて未押下であることを要求する。
検出可能なAPI失敗も入力せずerrorで終了する。snapshotの採否は純関数に分離して検査する。
GetAsyncKeyStateの0は未押下とAPI失敗を区別できないというWin32 API固有の限界は残る
（[Microsoft仕様](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getasynckeystate)で失敗時0、high bitが現在の押下を示すことを確認）。
これを時間窓や再試行で補わない。検査とSendInput間のOS状態の原子的な固定を保証するものではない。

**P2-2: pointerの前提は未立証、今回は規則を変更しない。** mouseの実測をpointerの実測と扱わない。
Microsoftの[NCPOINTERDOWN](https://learn.microsoft.com/en-us/windows/win32/inputmsg/wm-ncpointerdown)／
[NCPOINTERUP](https://learn.microsoft.com/en-us/windows/win32/inputmsg/wm-ncpointerup)は両方の既定処理で
system actionが起こり得るとしており、Down内での同期完結の証拠にはならない。
旧Upのhit再照会へは戻さない。同じ再照会の信頼性が実測で否定されたためである。
次回の実機確認では、メインPCのtouch panelで道具の手動モードを使い、最大化／復元の
tap、外で離す取消、caption操作を採取する（この作業中は起動しない）。penが使える場合は別logにする。

```powershell
Set-Location G:\home\mimageviewer-767s2\target\rm56-native-detached-probe
.\native_detached_probe.exe --log .\main-touch.log
$LASTEXITCODE
# penは別runで --log .\main-pen.log。終了は道具自身へのEscまたは×。
```

| 採取したいpointer列（いずれも未採取） | 現行結果 |
| --- | --- |
| NCPOINTERDOWN内で一致SCをdispatchし、そのDownが返却 | Completed |
| Down返却後、NCPOINTERUP内で一致SCをdispatch | Down返却でCaptionButtonOutside。後のSCでは復活しない |
| 昇格mouse内で一致SCをdispatch | raw Down call内なら一致SCを採用、raw Down返却後なら取消済みで再開しない。昇格NC mouse自体は別ownerにしない |

probeのwndproc／PM_REMOVE hookは全messageを記録し、NCPOINTERDOWN／UPDATE／UPの
high-word hitとlow-word pointer ID、POINTERCAPTURECHANGEDのIDを明示する。
GetCurrentInputMessageSourceのdevice／originまたはAPI errorを記録し、shared reducerのtraceも
実際に判定したNativeMouseOrigin、dispatch token、返却tokenを記録する。
pointerのUp／昇格mouseがどのcall期間でSCを起こしたかを照合する。採取成功の実機証拠はまだ無い。

**P2-3: 既知の残り。** 押下を伴わないAlt+Spaceの最大化、Win+Z等からのsnap選択には
再開の入口が無い。主差分以前からの入口不足であり、案1で列挙したmouse／touch操作とは別に
利用者の決定が必要なため、設計担当が別途相談する。この回ではコードを変更しない。

**再入の赤テスト。** 旧製品コード `e81ac0d94` に4件を追加し、すべて失敗を確認した。
commandは `cargo nextest run -p mimageviewer --lib -E 'test(/realmachine57_reentrant/)' --no-fail-fast --test-threads 6`、
nextest／run-heavy exit 100、証跡は `target/rm57r-red.log`。
pointerの別ID Down、mouseの同message Down、caption／resize Down、AwaitingReleaseの別ID Upで、
いずれも入れ子側の返却直後の「結果なし」という期待を破った。
修正後の最初の焦点は18実行／18成功、exit 0（`target/rm57r-focused.log`）。
最終版では4件が明示tokenを保存して返却する。さらにUpのhook先行有無×同ID／別ID、
ExitSizeMoveの同message再入も検査する。既存test seamの互換stackはtest-onlyであり、製品には無い。
独立source再レビュー（Sol / xhigh）は、P2-1／P2-4への追加P1・P2なし、ACCEPT。
P2-2／P2-3の見送りを解消済みとは扱わない。

最終gateは指定run-heavy、Owner `codex/767-rm57-review`、gate同士は直列。

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/realmachine57_|caption_button|native_detached_cancel_ends/)' --no-fail-fast --test-threads 6
cargo nextest run -p mimageviewer --lib -E 'test(/native_detached|non_client|caption|detached_activation|realmachine55_|realmachine57_|parked|native_video/)' --no-fail-fast --test-threads 6
cargo test -p mimageviewer --features dev-tools --bin native_detached_probe -- --test-threads 6
cargo build -p mimageviewer --features dev-tools --bin native_detached_probe
cargo check -p mimageviewer --bin mimageviewer-core
cargo fmt
```

| 検証 | 件数／exit code | 証跡 |
| --- | --- | --- |
| 旧コードでの再入赤テスト | 4実行／4失敗、exit 100 | `target/rm57r-red.log` |
| 最終焦点 | 19実行／19成功、exit 0 | `target/rm57r-focused-final.log` |
| 指定回帰 | 669実行／669成功、exit 0 | `target/rm57r-regression.log` |
| probe pure unit | 7実行／7成功、exit 0 | `target/rm57r-probe-unit.log` |
| probe build | 1 exe、exit 0 | `target/rm57r-probe-build.log` |
| core check | 1 core対象、exit 0 | `target/rm57r-check.log` |
| 最後の引数なしfmt | exit 0 | `target/rm57r-fmt.log` |

run-heavyはlock領域／G: TEMPへの書込不可の警告に従い、低優先度・6並列で続行した。
コピー用 `target/rm56-native-detached-probe/native_detached_probe.exe` を新buildへ差替えた。
33,461,248 bytes、SHA256 `7409a7c8a52a5ccb3cb4bf3b39cecb746b27c4f5687df02e48a802a55b3e4cea`。
build出力とのhash一致、6 DLLが旧manifestのhashのままであることを確認した。
exe＋DLLは151,274,512 bytes。PE／hash記録は `target/rm57r-pe-imports.json` と
同梱 `PE-IMPORTS.json`、READMEも更新した。DLL本体は変更しない。

変更ファイルは `src/key_input.rs`（scope／subclass／test driverと再入回帰）、
`src/key_input/native_detached_probe.rs`（共有trace）、`src/bin/native_detached_probe.rs`（preflightとpointer trace／unit）、
本書と `docs/detached-rework-plan.md` の5件。vendor／App field／通常配布物は変更しない。
未確認はpointerの実列・昇格、修正後の実OS動作とautoの実行。P2-3のkeyboard入口は未対応。
製品／probe／auto起動、全件gate、build-dev、commit／pushは行わない。

#### タッチ実測: buttonの所有者を昇格mouseへ統一（2026-11-15）

基点は `b325cc1ed`（2回目の独立レビューACCEPT）。観測者は利用者、メインPCのtouch panelで
道具の手動モードを操作した。根拠は `target/rm56-native-detached-probe/touch.log`（6,488行）、
手順は `C:/miv-subpc/steps-767-touch-probe.md`。窓が実際に最大化／復元した等の目視報告は未着。
以下はログから読める通知列であり、目視結果の代用とはしない。

| 操作／時刻 | ログ上の列と旧実装の結果 |
| --- | --- |
| 最大化button tap、約8.865s（4147行付近から） | raw NCPOINTERDOWN（HTMAXBUTTON、id=2）が約20msで返却しCaptionButtonOutside → raw Up → touch由来の昇格NCLBUTTONDOWN（hit=9）→ call内にCaptureChangedとSC_MAXIMIZE |
| 復元button tap、約16.615s | 同じraw→昇格列でSC_RESTORE |
| 指を外へずらす操作、約22.08s | client POINTERDOWN／UP。SC・outcomeなし |
| caption tap、約27.31s | raw NCPOINTERDOWN（HTCAPTION）→ raw Up返却でCompleted → 昇格Down内のSC_MOVE、追加outcomeなし |
| × tap、約31.06s | raw NCPOINTERDOWN（HTCLOSE）でCloseOwned → 昇格Down内のSC_CLOSE、追加outcomeなし |

raw NCPOINTERDOWNの既定処理内のNCHITTESTもHTCAPTIONを返しており、buttonのrelease hit再照会へは戻さない。
**P2-2の設計上の残りを閉じる。** タッチbuttonはraw pointerのDownでOSの選択が完了するという
旧仮定が実測で否定された。HTMINBUTTON／HTMAXBUTTON／HTHELPに限り、OSが選択を行う
昇格mouseのWM_NCLBUTTONDOWNを唯一のownerにする。

- raw NC pointer Downがbutton hitなら、bindingの取消／target差替えより前に対象外とする。
  raw Down自身の返却も対象外で、gestureもoutcomeも作らない。
- 昇格mouseのNCLBUTTONDOWNがbutton hitなら、既存 `NativePointerSource::Mouse` のownerを開始する。
  call内の一致SCだけで確定し、そのcall tokenの返却だけで終端する。SC無しならCaptionButtonOutside。
- invocationの既存token `NativeDetachedCall` にcaptured `NativeMouseOrigin` を保持する。
  subclassのdispatchとfinishはこの不変の出自と同じ純粋な所有判定を使う。
  native callから戻った時点の入力元を再取得しない。
- raw Upはhitだけで除外しない。captionで始め、button上で離した場合も、既存pointer IDのownerへ終端を返す。
- caption／resize／×はraw ownerを維持し、昇格mouseのDown／DoubleClick／Move／Upは別producerにしない。
  この初回実装ではbuttonの昇格で許可する開始をNCLBUTTONDOWNだけにした。
  後述の独立レビュー対応で通常mouseと同じNCLBUTTONDBLCLKも許可する。clientの経路は変更しない。
- 昇格が無ければbuttonのgesture自体が無い。rawと昇格を結ぶpending、相関、時間窓、retry、App fieldは追加しない。

「昇格mouseは別producerにならない」という従来規則のbuttonだけを、
「OSがbuttonを選ぶ唯一のcallが所有する」へ置き換えた（BA-7）。
mouseとtouchのbuttonは同じ既存owner／確定点を使い、raw／昇格を待ち合わせない。
開始hitが交差する仮定の列での二重outcomeとApp側の扱いは、後述の独立レビュー対応を参照。
ペンは未採取で、タッチと同じ昇格経路を使うと仮定して同じ規則を適用する。
タッチの修正後の実機確認も未実施。keyboard最大化の入口不足（P2-3）は従来どおり別件のまま残す。

**意味を更新したpointerテスト（3件）。**

| テスト | 更新内容 |
| --- | --- |
| `realmachine57_reentrant_pointer_down_returns_only_its_own_call` | raw buttonをownerにする前提を外し、raw caption／resize＋own Upで元Downの返却まで未終端を検査。pointer IDとcall identityの保護を維持 |
| `realmachine57_caption_button_raw_committed_action_finishes_at_down_return` | `realmachine57_caption_button_raw_calls_cannot_commit_without_promotion`へ変更。raw call内にSCがあってもbuttonのownerを作らない |
| `native_detached_raw_caption_button_tracks_only_own_pointer_target` | `native_detached_raw_caption_button_leaves_other_pointer_owner_unchanged`へ変更。他pointerのraw caption ownerをbutton Down／Upが変更しないこと、captionからbutton上へのreleaseは終わることを検査 |

実測列とその所有規則を検査する新3件は旧コードで3実行／3失敗、nextest／run-heavy exit 100を確認した。
commandは `cargo nextest run -p mimageviewer --lib -E 'test(/realmachine57_touch_button/)' --no-fail-fast --test-threads 6`、
証跡は `target/rm57t-red.log`。raw返却での不要な取消と、昇格Downだけの列でCompletedが無いことを検出した。
新たなcaption／closeの実測対照も加え、昇格後にoutcomeが二重にならないことを検査する。

最終検証は下表へ記録した。道具／auto／製品はこの作業で起動していない。

初回の修正後焦点ビルドはtest helper 1箇所のorigin引数更新漏れでcompile exit 101、テスト未実行
（`target/rm57t-focused.log`）。引数を補い、最終gateを実行した。
追加client対照の初回は23実行／22成功／1失敗、exit 100
（`target/rm57t-focused-before-client-api-fix.log`）。Release APIの戻り値にもoperation markerを
期待したテスト側の誤りで、markerを返すのはPressedだけだった。製品コードは変えず、
配送された `NativeClientInputBoundary::Released` が元のoperation IDを持つという既存契約で検査する。
独立source review（Sol / xhigh）はBA-7の構造修正としてACCEPT、追加P1／P2なし。

最終gateは指定run-heavy、Owner `codex/767-rm57-touch`、gate同士は直列。

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/realmachine57_|caption_button/)' --no-fail-fast --test-threads 6
cargo nextest run -p mimageviewer --lib -E 'test(/native_detached|non_client|caption|detached_activation|realmachine55_|realmachine57_|parked|native_video/)' --no-fail-fast --test-threads 6
cargo test -p mimageviewer --features dev-tools --bin native_detached_probe -- --test-threads 6
cargo build -p mimageviewer --features dev-tools --bin native_detached_probe
cargo check -p mimageviewer --bin mimageviewer-core
cargo fmt
```

| 検証 | 件数／exit code | 証跡 |
| --- | --- | --- |
| 旧コードでの新規3件 | 3実行／3失敗、exit 100 | `target/rm57t-red.log` |
| 最終焦点 | 23実行／23成功、exit 0 | `target/rm57t-focused-final.log` |
| 指定回帰 | 674実行／674成功、exit 0 | `target/rm57t-regression.log` |
| probe pure unit | 7実行／7成功、exit 0 | `target/rm57t-probe-unit.log` |
| probe build | 1 exe、exit 0 | `target/rm57t-probe-build.log` |
| core check | 1 core対象、exit 0 | `target/rm57t-check.log` |
| 最後の引数なしfmt | exit 0 | `target/rm57t-fmt.log` |

run-heavyはlock領域／G: TEMPへの書込不可の警告に従い、低優先度・6並列で続行した。
コピー用 `target/rm56-native-detached-probe/native_detached_probe.exe` を新buildへ差替え、
build出力とのSHA256一致を確認した。33,465,856 bytes、SHA256は
`ac3d99e98c55bfee58dfc82a7b9e4200d19be451b30779d4a854e743ef4bfc84`。
exe＋6 DLLは151,279,120 bytes。6 DLLは旧manifestのhashのまま、元touch.logも変更していない。
元touch.logのSHA256は `cd4e888c2f7c82978cda37769786fb0a5b08f15ed4528a25f0db4815f49cd857`。
PE記録 `target/rm57t-pe-imports.json`、同梱manifest／READMEを更新した。

変更ファイルは `src/key_input.rs` と本書、`docs/detached-rework-plan.md` の3件。
App field／vendor／通常配布物は変更しない。全件gate、build-dev、commit／pushは行っていない。

#### 依頼58: 最小化はOSに委ねて再開しない（2026-11-15）

基点は `c22de91e4`（タッチの所有変更をコミット済み）。観測者はサブPC担当、対象buildは
`b325cc1ed`。根拠は `C:/miv-subpc/results-58/notes.md` §4.4 と
`logs/mimageviewer.log` の221.915s／263.851s。止めたwindow=1／context=1の最小化buttonを
画面操作ツールとmouse_eventで各1回押すと、どちらもAccepted→commitとなり、
窓は元の大きさで前面に出て再開した（IsIconic=False）。動いている同窓の同位置では最小化された。
道具の最小化も旧結果はCompletedだった。

**原因のsource確認。** 旧native結果のCompletedは
`DetachedActivationRequest::native_outcome_diagnostic` でAcceptedとなり、
`sync_detached_native_input_targets` のsinkがactivationを配送してrootをwakeする。
service→commit→snapshot再開→`App::adopt_active_detached_viewport_runtime_from_passive`
（app.rs:66835、focus要求は66860）が `detached_viewer_focus_requested=true` にする。
active描画でこの要求をtake（ui_fullscreen.rs:27892）し、28282付近の
`request_detached_window_visibility(focus=true)` → `dispatch_detached_show`
（detached_placement.rs:120／128）でMinimized(false)とFocusを送る。
fallbackのui_fullscreen.rs:28284／28286も同じ復元／前面化を送る。
これは最小化を打ち消せるsource経路であり、実機ログの再開commitと整合する。

**利用者決定（2026-11-15、統括経由）。** Parked／ParkedLiveの最小化buttonは最小化だけを行い、
再開しない。×・button外releaseと並ぶ例外とする。タスクバーから戻す際の扱いは
master §1.790の別件で、この回では変更しない。

**実装。** 一致SCの選択確定・同Down call tokenの返却という境界は変えない。
既存 `ButtonActionCommitted` に閉じた `NativeButtonAction`（ActivateViewer／MinimizeOwned）を保持し、
HTMINBUTTON＋low 4 bitsを除いたSC_MINIMIZEだけをMinimizeOwnedにする。
HTMINBUTTON＋SC_RESTORE、MAX／HELPの一致SCは従来のCompleted。
raw pointer buttonは引き続きownerを作らず、touch／penの昇格mouseも同じ結果へ到達する。
選択SCなしは従来どおりCancelled(CaptionButtonOutside)。modal／retireの取消優先も変えない。

native outcomeとApp decisionの新名はともに **MinimizeOwned**。
normalizedは次の形になり、Accepted限定の既存輸送へ入らないため、accept／commitは発生しない。

```text
[detached-activation] boundary=normalized source=NonClientClick decision=MinimizeOwned reason=None window=... registration=... context=... expected_state=Parked
```

Appに最小化用flag／pendingを追加せず、focus要求を後からclearする処理も足さない。
安定したParked窓の通常描画は復元／前面化を要求しないことをreadonly監査した。
passive builderはlive HWNDへ初期配置を適用せず、minimized時はgeometryの保存を止めるだけ。
ほかの復元／focusは明示grid再open、F11、media presentationのFocusHost、trayなど各既存ownerの契機。
非detached専用restore_fullscreen_focus_from_mainはdetached sessionならreturnする。
mediaの同一host保守完了も `request.current==request.target` のためFocusHostを発行しない
（presentation_transition.rs:869）。ParkedLiveのsession確定は `parked_same_host` のearly returnで
active窓の公開を避ける（native_video.rs:5307／5335）。
最小化の結果を再開へ変換しなければ、今回の経路から別の自動restoreは起こさない。
最小化を遅延・抑止するguardや追加repaintは不要（BA-7）。

**回帰。** 新nativeテストは通常mouse／昇格mouseでSC_MINIMIZEの選択、focus／capture副作用、
SC返却ではなく本人Down返却でMinimizeOwnedとなることを検査する。
新Appテストは実grid openで作るParkedと既存実handlerのParkedLiveを使い、入力前の
admission=Acceptedを固定した上で、実native輸送→service→commitを通す。
active context／window／binding／runtime state／focus要求が変わらず、commitしないこと、
出力にMinimized(false)／Focusがないことを検査する。
初回fixtureのempty ROOT passはchild登録をGCし、旧コードでも入力が退役してしまったため、
そのpassを除き、登録が生きている境界で再検証した（製品の登録寿命は変更しない）。
旧コードではnativeのCompletedと、Appの実activation commitの両方で2実行／2失敗、exit 100
（`target/rm58-red.log`、初回fixture記録は `target/rm58-red-before-registration-fix.log`）。

最小化をCompletedとみなしていた更新対象は次の5件。外で離す取消、他button、call所有の期待は維持する。

| テスト | 変更 |
| --- | --- |
| `realmachine55_caption_button_native_command_finishes_at_outer_return_without_up` | MIN＋SC_MINIMIZEだけMinimizeOwned、MIN＋SC_RESTORE／MAX／HELPはCompleted |
| `realmachine55_caption_button_committed_minimize_keeps_its_native_focus_side_effect` | OSのfocus side effectを取消にせず、MinimizeOwnedで終端 |
| `realmachine57_touch_button_promoted_call_is_the_only_button_owner` | 昇格MINの結果だけMinimizeOwned、本人callまで未終端・1回だけは維持 |
| `native_detached_caption_button_outside_release_cancels_without_rect_change` | 正常選択する兄弟MINだけMinimizeOwned。対象のoutside取消は維持 |
| `native_detached_caption_button_far_motion_completes_with_os_command` | MINの選択結果をMinimizeOwnedへ |

独立source review（Sol / xhigh）は所有境界の構造修正としてACCEPT、阻害指摘なし。
検証は本項末尾へ記録した。道具／auto／製品はこの作業で起動していない。

最終gateは指定run-heavy、Owner `codex/767-rm58-minimize`、gate同士は直列。
別作業の全件gateが保有する共有lockの解放を待ってから開始し、他process／lockは変更していない。

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/realmachine57_|realmachine58_|caption_button/)' --no-fail-fast --test-threads 6
cargo nextest run -p mimageviewer --lib -E 'test(/native_detached|non_client|caption|detached_activation|realmachine55_|realmachine57_|realmachine58_|parked|native_video/)' --no-fail-fast --test-threads 6
cargo test -p mimageviewer --features dev-tools --bin native_detached_probe -- --test-threads 6
cargo build -p mimageviewer --features dev-tools --bin native_detached_probe
cargo check -p mimageviewer --bin mimageviewer-core
cargo fmt
```

| 検証 | 件数／exit code | 証跡 |
| --- | --- | --- |
| 旧コードでのnative／Appの赤テスト | 2実行／2失敗、exit 100 | `target/rm58-red.log` |
| タッチ＋最小化の最終焦点 | 25実行／25成功、exit 0 | `target/rm58-focused.log` |
| 指定回帰 | 676実行／676成功、exit 0 | `target/rm58-regression.log` |
| probe pure unit | 7実行／7成功、exit 0 | `target/rm58-probe-unit.log` |
| probe build | 1 exe、exit 0 | `target/rm58-probe-build.log` |
| core check | 1 core対象、exit 0 | `target/rm58-check.log` |
| 最後の引数なしfmt | exit 0 | `target/rm58-fmt.log` |

全件gateの後も共有heavy枠待ちがあり、指定runnerの待機を維持した。
コピー用 `target/rm56-native-detached-probe/native_detached_probe.exe` は新buildへ差替え済み。
33,466,368 bytes、SHA256は `57a554dc687ddd7d4742b00475815df1aee9b7bf2a9d2625a238c9eef74fdcf5`。
build出力とのhash一致、6 DLLと元touch.logが変更されていないことを検査した。
exe＋6 DLLは151,279,632 bytes。PE記録は `target/rm58-pe-imports.json`、同梱manifest／READMEも更新した。

タッチ側は `c22de91e4` として確定済みのraw／昇格の所有を維持し、この回の焦点・回帰に含めた。
タッチの3変更ファイルと今回の最小化の変更を合わせると、`src/key_input.rs`、
`src/app/detached_activation.rs`、本書、`docs/detached-rework-plan.md` の4ファイル。
App field／vendor／通常配布物は変更しない。修正後の最小化の実機動作、修正後touch、penの列は未確認。
元touch操作の目視報告は未着。タスクバー復元§1.790は対象外。
全件gate、build-dev、製品／道具／auto起動、commit／pushはこの作業から実行していない。

#### タッチ・最小化の独立レビューへの対応（2026-11-15着手）

基点は `9c69805bd`（タッチ `c22de91e4`、最小化 `9c69805bd`）。別会話の独立レビュー
（Sol / xhigh）はREJECT、OSが選ぶcallへの所有移動とMinimizeOwnedには構造修正として合意。
設計担当の決定でP2-2の昇格DBLCLKを修正し、P2-1のraw／昇格hit交差は追加状態を作らず
観測とAppの既存所有境界で扱う。以前の実装時source reviewのACCEPTは、このREJECTを置き換えない。

**P2-2。** 通常mouseはbuttonのDown／DBLCLKを所有する一方、昇格mouseの入口はDownだけだった。
raw buttonはownerを作らないため、昇格DBLCLK内にSCが来ても両方から取り残される。
昇格mouseもbutton hitの場合だけDown／DBLCLKを同じ所有判定へ通す。
既存のmouse owner、一致SC、captured originを含む本人call token返却を共用し、
SC_MAXIMIZE／SC_RESTOREはCompleted、SC_MINIMIZEはMinimizeOwnedを維持する。
button以外の昇格、raw buttonの除外、modal／retire、caption／resize／close／clientは変更しない。
rawと昇格を結ぶpending、時間窓、retry、App fieldは追加しない（BA-7）。

**P2-1の観測根拠。** 観測者は利用者、根拠は元の `target/rm56-native-detached-probe/touch.log`。
4tapのraw Downと昇格Downはすべて同HWND `0x12c193a`、同lParam、同hitだった。
以下は1始まりのwndproc行番号。全ペアはsource_device=4（touch）／source_origin=1。

| 操作 | raw NCPOINTERDOWN行 | 昇格NCLBUTTONDOWN行 | 共通lParam | 共通hit |
| --- | --- | --- | --- | --- |
| 最大化 | 4152 | 4241 | `0x7211bcb` | 9 |
| 復元 | 4301 | 4339 | `0x5f41e8b` | 9 |
| caption | 6146 | 6288 | `0x71b1aa2` | 2 |
| close | 6321 | 6455 | `0x7001c1c` | 20 |

対応するPM_REMOVE hook行は順に4147→4236、4296→4334、6141→6283、6316→6450。
raw CAPTION→昇格MAXのhit差、昇格DBLCLK、penの列はこの記録には無い。
これを全OS列の一般保証とは扱わず、設計担当は実測の開始点／hit一致とAppの既存排他を根拠に
raw／昇格の相関pendingを追加しないと決めた。

反例は `raw Down(CAPTION) → Down返却 → raw Up(MAX) → Up返却=Completed →
昇格Down(MAX) → SC_MAXIMIZE → 本人返却=Completed`。target同期を挟まなければnativeは2つの
outcomeを作り得る。これを無条件のnative producer唯一性と表現しない。
実handlerテストでは、同batchの2件目は既存Discrete intentによりCommandOwned、
最初のcommit後の取り出し済みrequestは期待Parked／ParkedLiveと現在stateの不一致でNotParkedとなった。
commit後に新たなnative入力を開始する場合はtarget同期でbindingが解除されるため、追加outcome自体が無い。
`realmachine58_review_cross_hit_results_apply_activation_once_without_turning_page` は
Parked／ParkedLive × 同batch／commit後の4組で、実native→sink→sharedから2requestが生成されること、
同じwindow／context／allocationを持つこと、commit成功が1回だけで次commitは無いこと、
owner／window／ページが維持されることを確認した。同batchは実serviceを通し、commit後は
取り出し済みrequestを実accept境界へ渡す。Appで問題は見つからず、設計担当の見送り判断を維持する。

**検証のやり直し（2026-11-16）。**
01:05頃のPC停止・resetを受け、前回の `target/rm58r-*.log` は今回の検証結果に採用しない。
焦点・回帰はrun-heavy経由の直列、nextestの `--test-threads 4` で流し直す。
全件／FullTest、製品／probe／auto起動、build-dev、commit／pushは実施しない。

App fixtureは通常の物理still openを通すため実current_folderを設定し、非main Cへのbindingをassertする。
独立Cには既存 `source_texture_test_support::seed` でcanonical表示sourceを準備し、
snapshot構築可能と現在ページ保持をhandoff前にassertしてから、通常grid openでParkedへ移す。
Parked・binding・activationの直接生成や、製品側の対処は追加しない。
検証結果・コマンド・件数・exit codeは以下へ記録する。

旧判定へ一時的に戻した赤確認は
`cargo nextest run -p mimageviewer --lib -E 'test(/realmachine58_review/)' --no-fail-fast --test-threads 4`。
3件／1成功／2失敗、exit 100（`target/rm58r2-red-valid.log`）。新DBLCLKの2件は
期待Completed／MinimizeOwnedに対し実outcomeが空で失敗し、Appの上記4組は成功した。
その後、共通開始判定をDOWN／DBLCLKへ戻して検証する。
最初の起動で `-j 1` と `--test-threads 4` がnextestの同一optionとして衝突し、実行0件／exit 2
（`target/rm58r2-red.log`）。余分な `-j` を外して上記を実行し、件数には含めない。

**修正後の検証（2026-11-16、reset後の再実行だけを採用）。**
すべて `C:/Users/mikag/AppData/Local/miv-locks/run-heavy.ps1`、
Owner `codex/767-rm58-review`、WorkDir `G:/home/mimageviewer-767s2` 経由で直列。
nextest／probe単体testは4並列。CARGO_INCREMENTAL=0、MSBUILDDISABLENODEREUSE=1。
runnerのlock／G: TEMP書込不可の警告時は、既定の低優先度・CARGO_BUILD_JOBS=6／既定TEMPで継続した。
本担当からFullTestや全件gateは実行しない。最後のCargo操作は引数なしのfmt。

| コマンド | 件数／結果 | exit | 証跡 |
| --- | --- | --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/realmachine57_\|realmachine58_\|caption_button/)' --no-fail-fast --test-threads 4` | 28／全成功 | 0 | `target/rm58r2-focus.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/native_detached\|non_client\|caption\|detached_activation\|realmachine55_\|realmachine57_\|realmachine58_\|parked\|native_video/)' --no-fail-fast --test-threads 4` | 679／全成功 | 0 | `target/rm58r2-regression.log` |
| `cargo test -p mimageviewer --features dev-tools --bin native_detached_probe -- --test-threads 4` | 7／全成功 | 0 | `target/rm58r2-probe-test.log` |
| `cargo build -p mimageviewer --features dev-tools --bin native_detached_probe` | build成功 | 0 | `target/rm58r2-probe-build.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | check成功 | 0 | `target/rm58r2-check.log` |
| `cargo fmt` | 整形成功 | 0 | `target/rm58r2-fmt.log` |

新規testは `realmachine58_review_promoted_button_doubleclick_maximize_completes_once`、
`realmachine58_review_promoted_button_doubleclick_minimize_keeps_os_owner`、上記cross_hitの3件。
既存minimize testは同じowner別fixtureを共用し、Parked／ParkedLiveで再開・復元・focusが出ない検査を維持した。
既存の期待をDBLCLK修正のために弱めていない。製品App／vendorは変更しない。
変更ファイルは `src/key_input.rs`、`src/app/detached_activation.rs`（testのみ）、本書、
`docs/detached-rework-plan.md` の4件。

コピー用 `target/rm56-native-detached-probe/native_detached_probe.exe` を新buildへ差し替えた。
33,443,840 bytes、SHA-256 `5d251af30bc998aa05805916a2a5d444a20a9683023409b6df96d6794f874a51`。
build出力とのhash一致、既存6 DLLのhash不変、元touch.log（6,488行、SHA-256
`cd4e888c2f7c82978cda37769786fb0a5b08f15ed4528a25f0db4815f49cd857`）の不変を確認した。
exe＋DLLは151,257,104 bytes。`target/rm58r2-pe-imports.json`、同梱PE-IMPORTS.json／README.txtを更新した。
配布物・DLL・元logは変更せず、道具は起動していない。

未確認は修正後の実機動作、実際のtouch／penの昇格DBLCLK列とpenの昇格自体。
P2-1の交差hit列も実測されておらず、上記4tapの一致を一般保証とは扱わない。
従来のkeyboard最大化入口とタスクバー復元§1.790は今回の対象外。
製品／probe／auto起動、全件gate、build-dev、commit／pushは行っていない。

実装担当内の別contextによる最終追レビュー（Sol / xhigh、2026-11-16）はACCEPT、追加P1／P2なし。
最終差分とreset後の上記証跡を照合し、Cargo／GUIの再実行はしていない。
これは今回の追補の確認であり、設計担当による受入判断の代替とはしない。

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
