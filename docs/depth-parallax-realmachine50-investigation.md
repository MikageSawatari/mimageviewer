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
| 外部openの通常main leaf（startup_ops:1806） | 共通grid handoffを迂回する。採用済みの宛先を使い、旧ownerのpark／close→新viewer公開を同じ確定点へ接続する |
| Bookmark既存Cの継続（:1638） | Preparing中の所有Cの処理。無条件のgrid handoffを入れて自己parkしない |
| Bookmark media新Cのinitializer（:2070） | 既に新しいloading ownerが予約されている。新規main openと区別し、継続は同じCで完了させる |
| 表示対象外のDirectMedia（:1797） | `MainListAfterHydration::DirectMedia` → `install_direct_media_reader` → required fullscreen loadへ迂回する。同型の所有移動を実装時に検証する。今回の赤テストは許可jpgで、この枝は未再現 |
| フォルダ／PDF／ZIPの列挙・deferred reopen | 新規閲覧の採用と、既に所有Cへ渡ったcontinuationを区別する。継続時に再度handoffしない。Bookmark、Collection、error/cancelも同じ所有区分で棚卸しする |

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
通常Fullscreenまでの正規hookはeframe側の変更を要する。**今回のvendor変更禁止を越えて実装しない。**
次段の設計検収で、backend hookを実装範囲へ含めるか、まずdetachedの既存正本への共用に限定するかを明示決定する。
後者の場合、通常Fullscreenのrect経路が残ることを完了扱いに混ぜない。

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

## 6. 診断の設計案（今回は追加しない）

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
