# 次版: 動画の戻る・進むボタンによるシーク再開計画

作成: 2026-09-09。対象は [backlog-on-hold.md](backlog-on-hold.md) §4.2。
本書は原因調査と入力 ownership の設計 checkpoint である。親と独立 reviewer の合意後、§4 の
behavior-neutral instrumentation と動画マウスボタンの小・中・大シーク 6 候補を実装した。
2026-09-09 の fresh trace により、標準 raw XButton と AHK 変換経路が別の OS 入力契約であることを
確定した。今回の恒久修正は標準 raw XButton の長押し ownership に限定し、AHK / browser-key /
direct AppCommand の重複は別課題として残す。

## 1. 現在確定している事実

22:08 確認用 build の利用者実行ログでは、1 回の物理操作から次の 2 receipt が App の
`handle_native_video_key_event` へ届き、各 receipt が別々に 1 回の seek action を実行した。

- 実 scan 付き `VK_BROWSER_BACK/FORWARD` (`scan_code=0x006A/0x0069`, `extended=true`)
- `scan_code=0`, `extended=false` の合成形 `VK_BROWSER_BACK/FORWARD`

下流で 1 receipt を二重適用した症状ではない。hold 中も両系列が `repeat=false` で届き、
通常の左右キーが初回 `repeat=false`、以降 `repeat=true` になる系列とは異なる。

一方、scan 0 receipt の producer は現ログから確定できない。20:15 build のログを根拠に
presenter `WM_KEYDOWN` 後の `DefWindowProcW` が同じ WndProc へ `WM_APPCOMMAND` を生成したと
判定し、browser keydown を処理済みで返す変更を入れたが、22:08 build でも症状と 2 receipt が
残った。このため、次のいずれかを新しい実ログなしに選んではならない。

- presenter WndProc が受けた直接 `WM_APPCOMMAND`
- HUD WndProc が受けた直接 `WM_APPCOMMAND`
- egui の native-video backdrop が `Event::Key` から作った scan 0 key
- main UI thread の `WH_GETMESSAGE` hook が観測した browser key / `WM_APPCOMMAND`
- 同じ OS message の複数回観測、または別 HWND / 別 message としての配送

近接した 2 行という事実だけを同じ物理入力の因果相関や重複判定には使わない。時間 debounce、
scan 0 の一律破棄、一定時間内の後着優先は、素早い 2 クリックと direct-APPCOMMAND-only 機器を
壊すため採用しない。

`target/next-version-work` の 2026-09-09 事前ログには実入力 provenance trace が無かったが、
候補再公開後に利用者が `MIV_MOUSE_SEEK_DEBUG=1` で fresh trace を採取した。AHK 有効時の
`current.log` は presenter HWND 1 個から scan 付き browser key DOWN / UP と direct
`WM_APPCOMMAND` が各 111 件届き、全 333 receipt は一意で render / App へ各 1 回だけ配送された。
98 組は DOWN→UP→AppCommand、13 組は DOWN→AppCommand→UP なので、UP の
`DefWindowProcW` だけを AppCommand の生成元とは断定できない。下流の同一 receipt 二重適用でもない。

利用者の SteelSeries / AHK v2 設定は F23 / F24 の反復 tap を Browser Back / Forward へ変換していた。
AHK を suspend / 終了した `ahk-disabled.log` では Extra1 / Extra2 の raw DOWN / UP / DBLCLK
だけになり、各 receipt は 1 回配送、1 click は 1 action、1.942 秒 hold も DOWN 1 件 + UP 1 件だった。
したがって標準機器の不足は重複除去でなく、raw hold を App が反復へ展開する ownership の欠落である。
AHK 経路の複数 OS message は別課題であり、時間 debounce、scan 0 / device 0 の破棄、direct
AppCommand の廃止では扱わない。

この hold 実装を含む最初の確認用 build は自動テストを通過したが、利用者の標準 raw XButton 実機確認では
single / rapid は成功し、hold は初回 action だけだった。`hold-failed.log` では generation 1 の DOWN が
App で seek を1回実行した後に hold を armせず、対応する UP は `release_unmatched` になった。原因は
`NativeVideoOutput.committed_generation` を現 presenter generation と誤解して完全一致を要求したことにある。
初期 presenter は event generation 1 で始まる一方、placement commit 前の値は 0 であり、この値は既存の
close 契約では stale event を捨てる下限である。テスト fixture も同じ accessor から event generation 0 を
作って実機の 1 / 0 境界を隠していた。自動テスト成功を実機 hold 成功とは扱わず、修正版 build の再確認を
完成条件として残す。

## 2. 現行 producer / consumer inventory

### 2.1 presenter / HUD の native route

`src/video/native_window.rs::wnd_proc` は presenter HWND の `WM_KEYDOWN/WM_SYSKEYDOWN` を
`NativeVideoKeyEvent` に変換し、lossless route へ送る。browser key は enqueue 後に
`DefWindowProcW` へ渡さない。`WM_APPCOMMAND` の command 1 / 2 は、それぞれ scan 0 の
browser back / forward keydown に変換して同じ route へ送る。raw XButton の
DOWN / UP / DBLCLK は `NativeVideoMouseButtonEvent` に変換し、全て処理済みで返す。

`src/video/native_window_host/hud_window.rs::hud_wnd_proc` も raw XButton を同じ mouse event に
変換し、直接 `WM_APPCOMMAND` は scan 0 browser keydown に変換する。presenter と HUD の
`NativeVideoWindowEventSink` は生成時点で `epoch`、`generation`、
`NativeVideoWindowSource::{Presenter,Hud}` を envelope に stamp する。mouse event はさらに
receiver HWND を持つ typed owner を保存する。DOWN は従来の overlay ownership を通す一方、Extra1 /
Extra2 の通常 UP と capture / cancel / destroy 時の synthetic UP は、受理済み generation の
lossless route から App の hold terminal へ必ず届く。presenter / HUD は reported button と実際に
capture を獲得した button を別集合で追跡し、複数 XButton は最後の capture-owned release だけが
`ReleaseCapture` する。非 X button の capture 失敗が後続 XButton の解放を妨げない。

`src/video/native_window_pump.rs` と `src/video/mod.rs` は source / epoch / generation を持つ
envelope を generation gate まで保ち、render thread から App へ bare `NativeVideoWindowEvent` を渡す。
key / mouse の required typed receipt は WndProc source、receiver HWND、元 message を保持し、mouse の
typed owner は window generation も保持する。App は同じ receipt id で producer / render / App ログを
相関できる。envelope の pump / render route-local sequence 自体は App payload に含めず、診断ログ側で
receipt id と結ぶ。

overlay は help、TextEdit、IME、modal、side panel 等の ownership を先に解決し、App へ渡すべき
キーだけを転送する。したがって計測は WndProc 受信、overlay disposition、App 到達を別々に記録し、
overlay が正当に所有した入力を「欠落」と扱わない。

### 2.2 native-video backdrop route

presenter HWND 未確定時は `src/ui_fullscreen.rs::native_video_key_events_from_ctx` が現在 viewport の
egui `Event::Key` を `NativeVideoKeyEvent` に作り直す。この変換では scan code と extended bit が
保持できず、常に scan 0 / non-extended になるが、required receipt の origin は `EguiBackdrop` と
viewport id を持つ。WndProc の `Win32Key` / `Win32AppCommand` と App ログでも区別できる。

### 2.3 main UI thread hook route

`src/lib.rs::mouse_nav_hook_proc` は main UI thread の `WH_GETMESSAGE` hook で
`WM_APPCOMMAND` と browser `WM_KEYDOWN/WM_SYSKEYDOWN` を観測し、方向別の process-global atomic
counter を増やす。`src/app.rs::handle_keyboard` と
`src/ui_fullscreen.rs::handle_fs_key_input` が mutually exclusive な条件で counter を drain し、
処理時点の focus / modal ownership を通して grid または viewer の mouse-button action へ渡す。

この hook には独立した未検証リスクがある。`GetMsgProc` の `wParam` は message を queue から
除去した `PM_REMOVE` と、除去せず観測した `PM_NOREMOVE` を区別するが、現コードは
`HC_ACTION` なら両方で counter を増やす。同じ queue message を `PM_NOREMOVE` で複数回見た場合、
複数 receipt を作り得る。ただし現行 native-video-key ログへ hook source は stamp されず、
detached の scan 0 receipt がこの route から来たとはまだいえない。また counter は個数を返す一方、
main と fullscreen の consumer は現在 `count > 0` へ縮約するため、同じ frame 内の正当な 2 click も
1 action になる。この cardinality 欠落も時間 dedup で直さず、trace 後の typed receipt queue で扱う。

Microsoft の契約:

- [`GetMsgProc`](https://learn.microsoft.com/en-us/windows/win32/winmsg/getmsgproc): hook の
  `wParam` は `PM_NOREMOVE` / `PM_REMOVE` を表す。
- [`WM_APPCOMMAND`](https://learn.microsoft.com/en-us/windows/win32/inputdev/wm-appcommand):
  `wParam` は command source HWND、`lParam` は command、device、key-state を含む。

### 2.4 最終 action と seek

`src/app/native_video.rs::dispatch_native_video_key_event` は 0xA6 / 0xA7 を動画 viewer の
back / forward mouse-button slot へ渡す。raw XButton は native overlay から App へ転送され、
同じ slot resolver へ合流する。main / image fullscreen の Extra1 / Extra2 と hook counter も
同じ `apply_mouse_back_forward_button` / `apply_mouse_button` 境界を使う。

slot が動画 seek を選んだ場合、Small / Medium / Large の段階は App の現在 Settings から秒数を
解決し、既存 player の relative seek へ 1 回だけ渡す。共有 dispatch は実際に実行した action を typed
result で返す。raw Extra1 / Extra2 の初回 DOWN が 6 種の seek を実行した場合だけ context-owned hold を
arm し、初回 action は即時に 1 回だけ実行する。Windows の keyboard delay / speed を arm 時に 1 回
sample し、delay 後は 1 frame / button あたり最大 1 回、取りこぼし分を burst せず同じ共有 executor へ
渡す。UP は対応 hold だけを停止する。Middle、非 seek action、browser key、direct AppCommand、
キーボード、ring、gesture、gamepad、tap、通常 wheel の独立経路は従来どおりである。

## 3. 守る不変条件

1. 物理 click 1 回は action 1 回、素早い click 2 回は action 2 回。
2. held input は採用した OS route の press / repeat / release contract に従う。release 後に
   pending action を残さず、keyboard browser key の bit 30 repeat を scan 0 の離散 command と
   混同しない。direct `WM_APPCOMMAND` に存在しない release を時間推測で捏造しない。
3. raw XButton、browser key、direct `WM_APPCOMMAND` の各単独機器を維持する。
4. grid の folder history / tree navigation、画像 viewer の mouse assignment、動画の
   keyboard / ring / gesture / gamepad / tap / wheel を維持する。
5. main、in-window、通常 fullscreen、F12 linked / independent、presenter、HUD、backdrop の
   owner を source HWND / viewport / epoch / generation で区別する。open、placement switch、
   source swap、modal open/close、viewer close 後の stale event を sibling context へ渡さない。
6. 設定に残る動画 mouse seek 値を deserialize / sanitize / save / cancel で消さない。
7. detached predicate、viewport identity / recreate、host ownership、placement / focus、window
   lifecycle に新しい bool / Option、時間窓、retry、blanket reset を追加しない。

## 4. 原因確定用の behavior-neutral instrumentation

親と独立 reviewer が本節を確認してから実装した。計測は環境変数
`MIV_MOUSE_SEEK_DEBUG` を明示したときだけ有効にし、通常時のログ量と挙動を変えない。
対象は browser back / forward、raw XButton の DOWN / UP / DBLCLK、および hold 比較用の
左右キー DOWN / UP に限定する。その他の keyboard / text / IME 入力は記録しない。1 process 内の
記録件数には固定上限を設け、上限後は cap 到達通知を一度だけ出す。ログ量抑制に時間窓を使わず、
入力の成立・dispatch 判定にもこの上限を使わない。

### 4.1 typed provenance

key と raw mouse button の両方に、optional field ではない共通
`NativeVideoInputReceipt { input_receipt_id, origin }` を持たせる。全 producer は typed
`NativeVideoInputOrigin` enum を必ず選ぶ。

- `Win32Key { window_source, receiver_hwnd, message }`
- `Win32AppCommand { window_source, receiver_hwnd, command_source_hwnd, command, device,
  key_state }`
- `Win32MouseButton { window_source, receiver_hwnd, message, button }`
- `Win32MouseCleanup { window_source, receiver_hwnd, cause_message }`
- `EguiBackdrop { viewport_id }`
- `Gamepad`

`Win32AppCommand` は high word の command / device と low word の key state を別 field として保存する。
未知 device bit も raw 値を失わない型にする。presenter / HUD の source は既存 sink stamp と一致を
assert できる形にし、テスト専用 producer は `#[cfg(test)]` constructor で明示する。

producer 行は receipt ごとに `input_receipt_id`, source kind, receiver HWND, message, vk, scan,
extended, repeat, AppCommand source HWND / device / key-state, epoch / generation を出す。
browser key と比較用左右キーは KeyDown だけでなく KeyUp も記録し、raw XButton は
DOWN / UP / DBLCLK と button id を残す。これにより hold 中の「repeat=false の完全な press/release
対が反復している」「一つの keydown が bit 30 repeat で継続している」「release receipt が無い」を
区別する。
WndProc が作る `input_receipt_id` は1物理入力の断定ではなく、decode 済みの1 message receiptを識別する
process-global単調IDとする。既存 `NativeVideoWindowEventEnvelope.sequence` は各 route の send 時に
採番される route-local ordering であり、pump / render へ clone された同じ receipt でも別系列になる。
ログでは `input_receipt_id` と `pump_route_sequence` / `render_route_sequence` を別名で出し、同じ意味に
使わない。既存 envelope の source / epoch / generation は generation gate で捨てる前に receipt と
結び付ける。

render disposition は同じ receipt id で overlay-owned / forwarded-to-App / stale-generation を出し、
App は handler 到達と、その後の routed / blocked outcome を同じ id に結ぶ。browser key / raw
XButton press は `action` と表記せず、mouse slot へ routed された事実だけを記録する。試用再公開後も
現行ログは標準 raw hold の初回 receiptと反復actionをbounded opt-in行で相関する一方、playerの
seek serial / deltaはreceipt payloadへ入れない。標準raw routeは一意receiptの実測、typed dispatch結果、
exact seek regressionで根因境界を確定した。AHK / browser-key / AppCommandの複数OS messageを将来まとめる
場合は、実seekのserial before / afterも同じreceiptへ結ぶことを追加受入条件とし、「受信した2 OS message」
「同じmessageの複数観測」「1 eventの二重転送」を区別できるようにする。

backdrop は OS provenance を推定せず `EguiBackdrop` とだけ記録する。egui event がどの Win32
messageから作られたかは、同じ時刻ではなく hook / main subclass の HWND・message receipt と
突き合わせて判断する。現行 backdrop dispatch は `pressed=true` だけである。KeyUp は診断専用に
観測するが `NativeVideoKeyEvent` として dispatchせず、既存の pressed-only predicateを維持する。

### 4.2 hook observation

hook は候補 filter を先に通し、browser back / forward と比較用の左右 key DOWN / UP、command
1 / 2 の `WM_APPCOMMAND` 以外を形式化も記録もしない。対象 message について
`hook_wparam=PM_REMOVE|PM_NOREMOVE|unknown`、queue message HWND、message、VK または AppCommand
raw high word / command / device flags / low-word key stateを記録する。この計測段階では counter
条件を変えない。`MIV_MOUSE_SEEK_DEBUG` は `OnceLock` で一度だけ読み、空文字と
`0` / `false` / `off` / `no` は無効として扱う。固定上限後は「以後を破棄する」という cap 到達通知を
process で一度だけ出し、実数の drop 件数を表明しない。`PM_NOREMOVE` でも counter を増やす現コードの重複可能性は Microsoft の契約と
pure handler test から決定的に示せるため、reported native scan 0 の原因確定とは分けて修正できる。
その場合は `PM_REMOVE` だけを canonical queue receipt とする最小修正と、per-HWND typed route へ
置き換える構造変更を比較し、main / fullscreen / modal の drain ownership を壊さない方を独立
checkpoint で確定する。いずれの修正結果も native presenter の reported cause 解消とは表明しない。

### 4.3 write ownership

計測スライスで変更する予定の production files は次に限定する。

- `src/mouse_seek_debug.rs`: 共通 typed receipt / origin、候補 filter、bounded logger と pure decode tests
- `src/video/native_window.rs`: presenter key / AppCommand producer、decode / route tests
- `src/video/native_window_host/hud_window.rs`: HUD producer と AppCommand metadata tests
- `src/video/mod.rs`: envelope disposition / UI forward の receipt logging と route tests
- `src/ui_fullscreen.rs`: backdrop producer の明示
- `src/app/native_video.rs`: App outcome correlation と handler tests
- `src/app/gamepad_input.rs`:既存 synthetic native-key producer の `Gamepad` 明示
- `src/lib.rs`: `WH_GETMESSAGE` の removal disposition を含む opt-in observation と pure decode tests
- `src/keymap.rs`, `src/video/native_presenter/render_core.rs`, `src/app/tests.rs`: required receipt の
  test fixture 対応

ドキュメントは本書を正本とし、原因確定後に `docs/video-architecture.md`、
`docs/keymap-spec.md`、必要なら `docs/detached-rework-plan.md` §11を完成設計へ更新する。
この checkpoint では `settings.rs`、`ring_shortcut.rs`、設定 UI、manual を変更せず、候補を戻さない。
`src/ui_fullscreen.rs` は背景色担当と共有するため、計測側が backdrop origin の小変更を先に完了し、
対象行と diff を親へ示して明示 handoff した後は、背景色担当の exclusive ownership に戻す。

### 4.4 利用者指示による検証用再公開

次版の検証 build では `RingActionId::is_available_for_mouse_button_assignment` を既存の
`is_valid_for_mouse_button_context` と同じ契約へ戻す。これにより action-first / slot-first の
操作カスタマイズ UI と共有 `App::apply_mouse_button` が同じ正本から 6 候補を表示・実行する。
保存 schema、既定割り当て、シーク秒数、他 context の候補、location navigation 除外は変えない。

これは検証を可能にした policy rollback であり、この時点では receipt queue、counter、producer、
WndProc、scan code、hold / repeat、focus、epoch / generation、detached lifecycle を変更しなかった。
後続の fresh trace で標準 raw XButton と AHK 変換経路を分離し、標準 raw hold だけを §6 の構造で
恒久化した。AHK / browser-key / AppCommand の複数 OS message、同一 frame の hook count 縮約、
AppCommand に release が無い制約は残る。

## 5. fresh trace のシナリオと判定

自動テストと build の後、実入力を伴う確認は利用者が内容・所要時間・desktop/input 使用を
明示承認した別枠だけで行う。agent が通常 profile の executable を起動しない。通常 profile の
実データを使うため、installed / tray-resident mImageViewer を閉じてから利用者自身が repository root で
次を実行する。環境変数は起動した child に継承され、現在の PowerShell からは直後に除く。

```powershell
$env:MIV_MOUSE_SEEK_DEBUG = '1'
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
Remove-Item Env:MIV_MOUSE_SEEK_DEBUG
```

同じ物理機器について back / forward 各方向で次を採取する。

1. 単発 click 3 回。各 gesture の間を明確に空ける。
2. 素早い 2 click を 3 組。
3. hold し、最初の action、反復開始、反復間隔、release 後の停止を採取する。
4. main grid、通常 fullscreen video、F12 video で比較する。
5. presenter canvas 上と HUD 上、modal / TextEdit 所有中、presenter 未確定 backdrop が実際に
   観測できる場合を区別する。

direct-APPCOMMAND-only 機器を実機で用意できない場合、Win32 handler-level test で direct message
1 件が provenance 付き action 1 件になる fallback 契約を固定する。実在機器での確認済みとは
表明しない。

検証用 build で producer / route receipt の種類と個数、hold / release系列を採取した結果、標準raw
XButtonは各receiptが1回だけroutedされ、AHK有効時だけ別OS messageが併存した。標準raw routeはこの一意性と
handlerのexact relative-seek regressionを合わせて完成根拠にする。AHK / browser-key / AppCommand側を修正する
将来stageでは、各receiptのrouted outcomeと実seek serial / deltaを同じidへ結ぶinstrumentationを追加条件とする。

原因確定には、2 App receipt の各々を producer の source kind、receiver HWND、message、
AppCommand device/key-state、route receipt id まで遡れることを要求する。次の例は互いに別修正である。

- presenter / HUD が独立した key と AppCommand を受信: OS metadata と HWND relation に基づき、
  canonical message owner をその WndProc 境界で決める。
- backdrop と native presenter の両方が同じ入力を処理: viewport / host transition の既存 owner を
  一つだけ選び、stale sibling への転送を止める。
- hook の `PM_NOREMOVE` 再観測: hook の queue-removal contract または per-HWND typed receipt owner を直す。
- App / render route が同じ receipt id を複数 forward: generation-stamped lossless route の一意配送を直す。

修正箇所は fresh trace が示した ownership boundary だけに絞る。source provenance を持たない
scan 0 判定、時刻の近さ、focus の事後状態は修正条件にしない。

## 6. fresh trace 後の恒久化境界

標準 raw XButton は presenter / HUD の typed owner、App の projected `ViewerContextId`、fs_idx、source
epoch、window generation、source HWND、press 時の slot / action を 1 つの hold に保存する。
window generation は現在の presenter / HUD HWND と厳密に照合し、`committed_generation` は exact current
identity ではなく既存 close 契約と同じ stale floor として `owner_generation >= committed_generation` を
要求する。これにより初期 generation 1 / committed floor 0 を受理し、同じ HWND が再利用されても floor
未満の古い generation を拒否する。新しい parallel generation state は追加しない。
Back / Forward は独立 slot なので同時押下でも上書きしない。通常 frame 間の bundle `AtRest` は同じ
context の保存形であり hold を保持し、次 mount で全 stamp と現在割り当てを再検証して継続する。
context retire / drop、別 session への transfer / ParkedLive、native output close、source swap、placement
replace、fullscreen item change、remote / audio / VST への所有交代は terminal として clear する。

`poll_video` は native event batch を先に適用し、その後に hold tick を 1 回だけ行う。これにより同じ batch
の UP が repeat より先に停止を確定する。期限前は既存 repaint deadline へ残り時間を merge し、期限超過は
1 回実行後に観測時刻 + interval へ進める。追加 thread、sleep、同期 I/O、catch-up queue は作らない。

AHK / browser-key / direct AppCommand は release を共有しない別 producer 契約なので今回の hold owner へ
混ぜない。AHK 有効 trace の別 OS message を恒久的にまとめるには hook の `PM_REMOVE` ownership と
receipt queue / action correlation を含む別設計が必要であり、標準 raw XButton の完成条件にはしない。
保存 schema と seek 秒数 field、候補 UI、既存 mouse-action resolver は変更しない。

## 7. regression と gate

原因計測と候補再公開の既存回帰に加え、標準 raw XButton の完成条件を次で固定する。

- initial DOWN は即 1 action、DBLCLK は独立した2回目、UP 後は repeat しない。
- 期限超過でも 1 frame / button 1回だけで、次 deadline は未来へ進む。Windows delay / speed の
  0..3 / 0..31 clamp と API failure fallback を固定する。
- 6 seek だけが arm し、非 seek、audio / VST / remote / ParkedLive、別 fs_idx / HWND / epoch /
  generation、割り当て変更は停止する。generation は実 event owner を明示した fixture で、初期
  generation 1 / committed floor 0、同値 1 / 1、stale 1 / 2 を固定する。
- Extra1 / Extra2 を同時に保持し、一方の UP が他方や Win32 capture を解放しない。
- overlay ownership が DOWN 後に変わっても通常 / synthetic UP は lossless App terminal へ届く。
- presenter / HUD の capture failure・capture change・cancel・destroy cleanup と、非 X stale reported bit が
  後続 XButton capture release を妨げない契約を固定する。
- `ViewerContextBundle` の通常 AtRest round-trip は hold を保持し、ParkedLive transfer / context close は
  terminal clear する。main / F12 の sibling owner は混線しない。
- 共有 typed dispatch が初回に実行した 6 action と現在 Settings の秒数を repeat も再利用する。

焦点検証後に core check、format check、共有入力変更として `scripts/test-full.ps1`、最後に
`scripts/build-dev.ps1` を一人の検証担当が実行する。通常 profile build は agent が起動せず、
利用者へ具体的な実機シナリオと起動時の実データ注意を渡す。

## 8. detached rework との関係

標準 raw hold は既存 `ViewerContextBundle` に typed state を加え、通常の mounted / AtRest swap を
そのまま state owner として使う。detached predicate、viewport identity / recreate、host create / destroy、
placement / focus routing は変えない。terminal は既存 context / source / presentation ownership の交代点で
clear し、時間窓、detached 専用 guard、App-global fallback state を追加しない。

親と独立 reviewer は着手前に、main / F12 の同じ bundle lifecycle へ入力 state を置く構造修正であり、
§2 が禁じる症状パッチではないと合意した。完成範囲を `docs/detached-rework-plan.md` §11へ記録する。
AHK / hook の未解決 producer 契約はこの state へ推測で合流させず、別の coherent stage とする。
