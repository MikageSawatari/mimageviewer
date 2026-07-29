# Native video HWND ownership / pump 分離計画

## 0. 目的と結論

対象は `docs/next-release-backlog.md` §1.27 (P1) である。2026-07-29 の
`cdb -pv` 実測では、UI thread が破棄中の viewer HWND の配下にある
`mIVNativeVideoWindow` へ同期 message を送ったまま待ち、その HWND を所有する
`native-video-presenter` thread は D3D11 driver 内で待っていた。これは keyed mutex
固有の問題ではなく、**message pump の可用性と、時間上限を保証できない GPU work を同じ
thread に載せた ownership 違反**である。

本計画の推奨は **案 B: native video window pump 専用 thread と render thread の分離**
である。pump thread が全 placement の `mIVNativeVideoWindow` と HUD HWND を生成・所有・
破棄し、render thread は D3D11 / DXGI / DirectComposition、frame fence、keyed mutex、
present だけを所有する。pump は render、decoder、audio、VST bridge の完了を待たない。

この変更は detached の矩形判定、viewport 再生成、App-global flag を増やすものではない。
`mIVNativeVideoWindow` が child か popup かを問わず「HWND owner は message pump を
回し続ける」という ownership boundary を全経路で成立させるため、症状パッチではなく
構造的修正である。実装時は
[detached rework plan §11](detached-rework-plan.md#11-変更履歴) に記録する。

本書は設計のみであり、production code の変更は含まない。

## 1. 調査の前提

### 1.1 失敗と守るべき条件

実測済みの失敗は次の相互待ちである。ここでは再現条件や stack の再調査は行わない。

1. UI thread が viewer HWND を破棄する。
2. viewer の child `mIVNativeVideoWindow` は別の
   `native-video-presenter` thread が所有している。
3. Win32 が破棄・活性化に伴う同期 message を child owner thread へ送る。
4. presenter thread は D3D11 `AcquireSync` 内部の `Flush` / driver wait に入り、
   message を dispatch できない。
5. UI thread と presenter thread のどちらも進まず、アプリ全体が復帰不能になる。

Windows では window を作成した thread がその window を所有し、message loop を提供する。
また、parent/owner の破棄は child/owned window の破棄を伴う。Microsoft の
[Creating Windows in Threads](https://learn.microsoft.com/en-us/windows/win32/procthread/creating-windows-in-threads)、
[DestroyWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-destroywindow)、
[About Messages and Message Queues](https://learn.microsoft.com/en-us/windows/win32/winmsg/about-messages-and-message-queues)
も、この ownership と message dispatch を前提にしている。

### 1.2 用語

- **UI thread**: `eframe::run_native` と全 winit / egui viewport を動かす main thread。
- **pump thread**: 本計画で新設する、native video / HUD HWND の唯一の owner。
- **render thread**: D3D11 / DXGI / DirectComposition と frame 提示を担当する thread。
  現行の `native-video-presenter` の GPU 部分を継承する。
- **host HWND**: `mIVNativeVideoWindow` の parent または owner になる main / detached
  viewer HWND。
- **placement**: 既存 `NativeVideoPlacement` の
  `MainWindowChild` / `FullscreenBorderless` / `DetachedViewerChild` /
  `DetachedWindow`。
- **request id / epoch**: window request と render target の世代を相関させる単調増加 ID。
  HWND 値の再利用を同一 window と誤認しないために使う。

## 2. 現状の ownership map

### 2.1 HWND と thread の対応

次表の「生成 / pump」は production source から確認した関係である。`Default IME` /
`MSCTFIME UI` はアプリが明示的に生成する window ではなく、入力対象となる GUI thread に
Windows / TSF が暗黙生成する。presenter thread 上に存在したことは今回の debugger 実測、
UI / VST GUI thread 上の存在可能性は Win32/TSF の性質からの帰結であり、明示生成の
source location はない。

| HWND / mode | 生成・所有・pump thread | parent / owner | source evidence | owner thread が block し得る処理 |
| --- | --- | --- | --- | --- |
| main winit HWND | UI thread | top-level | `eframe::run_native` は [src/lib.rs:994](../src/lib.rs#L994)-[1052](../src/lib.rs#L1052)。HWND capture は [src/app.rs:59490](../src/app.rs#L59490)-[59511](../src/app.rs#L59511) | egui update、OS message dispatch。同期 I/O は repo 規約で禁止されており、本件の通常経路では GPU / decoder / audio wait を持たない |
| detached / fullscreen egui viewport HWND | UI thread | independent top-level。detached builder は taskbar window で明示 parent を持たない | viewport 作成・registry 登録は [src/ui_fullscreen.rs:6326](../src/ui_fullscreen.rs#L6326)-[6409](../src/ui_fullscreen.rs#L6409)、host capture は [src/app.rs:34540](../src/app.rs#L34540)-[34596](../src/app.rs#L34596)、builder は [src/ui_fullscreen.rs:10282](../src/ui_fullscreen.rs#L10282)-[10372](../src/ui_fullscreen.rs#L10372) | main と同じ。viewport の close / recreate で、その child/owned HWND との同期 message が発生し得る |
| `mIVNativeVideoWindow`: `MainWindowChild` | `native-video-presenter` | `WS_CHILD`、parent = main HWND | placement 判定は [src/video/mod.rs:265](../src/video/mod.rs#L265)-[300](../src/video/mod.rs#L300)、mode/owner mapping は [src/video/mod.rs:1891](../src/video/mod.rs#L1891)-[1918](../src/video/mod.rs#L1918)、style/create は [src/video/native_window.rs:407](../src/video/native_window.rs#L407)-[455](../src/video/native_window.rs#L455) | §2.2 の GPU / DComp / frame wait 全て |
| `mIVNativeVideoWindow`: `DetachedViewerChild` | `native-video-presenter` | `WS_CHILD`、parent = detached host HWND | 同上。detached host の選択は [src/app.rs:38403](../src/app.rs#L38403)-[38423](../src/app.rs#L38423) | 同上。今回実測した相互待ちの構成 |
| `mIVNativeVideoWindow`: `FullscreenBorderless` | `native-video-presenter` | `WS_POPUP`、owner = main HWND | mode は [src/video/native_window.rs:138](../src/video/native_window.rs#L138)-[157](../src/video/native_window.rs#L157)、mapping/create は上記 | 同上。child ではなくても owned popup の activation / focus / destroy が owner UI と相互作用する |
| `mIVNativeVideoWindow`: `DetachedWindow` | `native-video-presenter` | standalone `WS_POPUP`、owner = null | owner mapping は [src/video/mod.rs:1909](../src/video/mod.rs#L1909)-[1918](../src/video/mod.rs#L1918) | 同上。UI-owned parent との破棄環はないが、自身の close / IME / focus message が停止する |
| placement 切替中の新旧 `mIVNativeVideoWindow` | 同じ `native-video-presenter` | placement ごとの parent / owner。新 window を hidden で作り prime 後に旧 window を破棄 | rebuild は [src/video/mod.rs:2841](../src/video/mod.rs#L2841)-[3205](../src/video/mod.rs#L3205)、新規作成 [2921](../src/video/mod.rs#L2921)-[2969](../src/video/mod.rs#L2969)、GPU prime [3064](../src/video/mod.rs#L3064)-[3100](../src/video/mod.rs#L3100)、swap/show/destroy [3117](../src/video/mod.rs#L3117)-[3157](../src/video/mod.rs#L3157) | 新旧 HWND を所有したまま GPU prime / present / DComp wait を実行する |
| native video HUD HWND | `native-video-presenter` | `WS_POPUP`、owner = fullscreen presenter HWND。focus は presenter へ戻す | fullscreen のみ有効化 [src/video/mod.rs:1922](../src/video/mod.rs#L1922)-[1928](../src/video/mod.rs#L1928)、presenter 内生成 [src/video/native_presenter/mod.rs:1639](../src/video/native_presenter/mod.rs#L1639)-[1688](../src/video/native_presenter/mod.rs#L1688)、thread-affinity contract [src/video/native_presenter/hud_window.rs:87](../src/video/native_presenter/hud_window.rs#L87)-[159](../src/video/native_presenter/hud_window.rs#L159)、CreateWindowEx [161](../src/video/native_presenter/hud_window.rs#L161)-[265](../src/video/native_presenter/hud_window.rs#L265) | presenter HWND と同じ GPU waits。HUD 自体の mouse dispatch / hit test も同時に止まる |
| presenter thread の `Default IME` / `MSCTFIME UI` | Windows / TSF が presenter GUI thread に暗黙生成。今回 debugger で実測 | presenter input context に付随 | native wndproc が IME を処理 [src/video/native_window.rs:985](../src/video/native_window.rs#L985)-[1048](../src/video/native_window.rs#L1048)、presenter を focus [src/video/mod.rs:2106](../src/video/mod.rs#L2106)-[2113](../src/video/mod.rs#L2113) | presenter thread の GPU waits。IME preedit / commit / focus transition も停止する |
| UI thread の `Default IME` / `MSCTFIME UI` | Windows / TSF が winit/egui input GUI thread に暗黙生成し得る | main / viewport input context に付随 | アプリによる明示 create はない | UI update / message dispatch。別 thread の child/owned HWND への同期 transition に巻き込まれる |
| VST editor container | VST3 bridge process の slot ごとの STA GUI thread | `WS_POPUP`、owner = main HWND または fullscreen presenter HWND | owner selection は [src/app/native_video.rs:1862](../src/app/native_video.rs#L1862)-[1888](../src/app/native_video.rs#L1888)。container create は [crates/vst3-host/src/plugin_loader.cpp:490](../crates/vst3-host/src/plugin_loader.cpp#L490)-[573](../crates/vst3-host/src/plugin_loader.cpp#L573)、GUI loop は [720](../crates/vst3-host/src/plugin_loader.cpp#L720)-[845](../crates/vst3-host/src/plugin_loader.cpp#L845) | plugin `DispatchMessage` / editor callback / COM。bridge の明示 GUI task は timeout で bridge process を終了するが、第三者 plugin code の停止可能性は残る |
| VST plugin host child | 上と同じ bridge slot GUI thread | `WS_CHILD`、parent = VST editor container | [crates/vst3-host/src/plugin_loader.cpp:490](../crates/vst3-host/src/plugin_loader.cpp#L490)-[573](../crates/vst3-host/src/plugin_loader.cpp#L573) | 上と同じ |
| VST GUI thread の IME windows | Windows / TSF が plugin editor の STA GUI thread に暗黙生成し得る | plugin editor input context に付随 | `OleInitialize` と message loop は [crates/vst3-host/src/plugin_loader.cpp:720](../crates/vst3-host/src/plugin_loader.cpp#L720)-[845](../crates/vst3-host/src/plugin_loader.cpp#L845)。アプリによる IME HWND の明示 create はない | plugin GUI callback / COM |

`NativeVideoWindow` と HUD は production では同じ presenter thread から生成される。
`run_native_video_output` の thread spawn は [src/video/mod.rs:836](../src/video/mod.rs#L836)-
[940](../src/video/mod.rs#L940)、window/presenter の生成は
[src/video/mod.rs:1932](../src/video/mod.rs#L1932)-[2030](../src/video/mod.rs#L2030) にある。
loop は一周の冒頭で一度だけ `PeekMessage` pump を行い
([src/video/mod.rs:2353](../src/video/mod.rs#L2353)-[2370](../src/video/mod.rs#L2370))、
その後の work が戻るまで次の message を処理できない。

### 2.2 block し得る経路

次の処理は現在の presenter thread で実行される。API timeout が見える箇所でも、driver 内部の
flush や kernel transition まで有限時間を保証するものではない。

| 経路 | source evidence | HWND owner との同居による問題 |
| --- | --- | --- |
| frame-latency wait | [src/video/native_presenter/mod.rs:2099](../src/video/native_presenter/mod.rs#L2099)-[2126](../src/video/native_presenter/mod.rs#L2126) | 100 ms 単位でも message latency を作り、driver failure では上限を保証できない |
| swapchain `Present` | 同上、および [src/video/mod.rs:2662](../src/video/mod.rs#L2662)、[2880](../src/video/mod.rs#L2880)、[3083](../src/video/mod.rs#L3083)、[3093](../src/video/mod.rs#L3093)、[3097](../src/video/mod.rs#L3097)、[4112](../src/video/mod.rs#L4112) | DXGI / DWM / driver の同期経路に入る |
| D3D11 fence `Wait` | [src/video/native_presenter/mod.rs:2497](../src/video/native_presenter/mod.rs#L2497)-[2515](../src/video/native_presenter/mod.rs#L2515) | decoder/producer GPU work の完了を待つ |
| keyed mutex `AcquireSync` | [src/video/native_presenter/mod.rs:2521](../src/video/native_presenter/mod.rs#L2521)-[2529](../src/video/native_presenter/mod.rs#L2529)、helper は [3620](../src/video/native_presenter/mod.rs#L3620)-[3648](../src/video/native_presenter/mod.rs#L3648) | 今回 `AcquireSync(0, 10 ms)` 内部の `Flush` が 75 秒以上戻らなかった。指定 timeout は pump の生存保証にならない |
| DirectComposition commit completion / `DwmFlush` | init は [src/video/native_presenter/mod.rs:1965](../src/video/native_presenter/mod.rs#L1965)-[1983](../src/video/native_presenter/mod.rs#L1983)、surface swap は [2163](../src/video/native_presenter/mod.rs#L2163)-[2329](../src/video/native_presenter/mod.rs#L2329)、wait helper は [3554](../src/video/native_presenter/mod.rs#L3554)-[3569](../src/video/native_presenter/mod.rs#L3569) | compositor / DWM の応答を待つ |
| placement rebuild の target create / prime / old target drop | [src/video/mod.rs:2841](../src/video/mod.rs#L2841)-[3205](../src/video/mod.rs#L3205) | 新旧 HWND を所有したまま複数の GPU/DComp 操作を連続実行する |

decoder と audio は別 thread であり、現状 HWND を所有しないため、直接の Win32 ownership
違反ではない。ただし設計境界を誤って pump 側へ移さないため、block 点を明示する。

| subsystem | thread / block point | source evidence |
| --- | --- | --- |
| demux / decoder | `video-demux` と audio/video decoder threads。file / packet read、driver decode、packet/frame queue backpressure | spawn は [src/video/decoder.rs:1710](../src/video/decoder.rs#L1710)-[1730](../src/video/decoder.rs#L1730)、decode thread は [2574](../src/video/decoder.rs#L2574)-[2597](../src/video/decoder.rs#L2597) と [2666](../src/video/decoder.rs#L2666)-[2690](../src/video/decoder.rs#L2690)、receive timeout は [3503](../src/video/decoder.rs#L3503)-[3512](../src/video/decoder.rs#L3512) |
| audio | audio pump の queue wait / VST process、cpal real-time callback の buffer access | spawn/callback は [src/video/audio.rs:647](../src/video/audio.rs#L647)-[703](../src/video/audio.rs#L703)、pump select は [917](../src/video/audio.rs#L917)-[943](../src/video/audio.rs#L943)、VST processing は [1165](../src/video/audio.rs#L1165)-[1273](../src/video/audio.rs#L1273) |
| VST bridge IPC | bridge response deadline と process wait | [src/video/dsp/bridge.rs:896](../src/video/dsp/bridge.rs#L896)-[1041](../src/video/dsp/bridge.rs#L1041) |

pump はこれらから data / notification を受けても、同期完了を待ってはならない。

### 2.3 現行 lifecycle

- `post_quit_on_destroy` は `DetachedViewerChild` だけ false、それ以外は true
  ([src/video/mod.rs:1963](../src/video/mod.rs#L1963)-[1984](../src/video/mod.rs#L1984))。
- wndproc は `WM_CLOSE` で close event を送り `DestroyWindow`、`WM_DESTROY` で flag が true
  なら `PostQuitMessage` する
  ([src/video/native_window.rs:1216](../src/video/native_window.rs#L1216)-[1245](../src/video/native_window.rs#L1245))。
- placement 切替時は `destroy_silent` が一時的に quit posting を抑える
  ([src/video/native_window.rs:542](../src/video/native_window.rs#L542)-[581](../src/video/native_window.rs#L581))。
- output cleanup でも presenter thread 自身が HWND を破棄する
  ([src/video/mod.rs:4509](../src/video/mod.rs#L4509)-[4525](../src/video/mod.rs#L4525))。
- `NativeVideoOutput` drop は cancel を送り、join 自体は background join thread に逃がしている
  ([src/video/mod.rs:1275](../src/video/mod.rs#L1275)-[1303](../src/video/mod.rs#L1303))。これは UI
  join を避けるが、HWND owner の pump 停止は解消しない。

wndproc は単なる forwarding ではない。

- child activation / foreground 判定: [src/video/native_window.rs:882](../src/video/native_window.rs#L882)-[929](../src/video/native_window.rs#L929)
- Escape / key event / `event_tx`: [src/video/native_window.rs:931](../src/video/native_window.rs#L931)-[951](../src/video/native_window.rs#L951)
- `WM_CHAR` / IME preedit / commit: [src/video/native_window.rs:985](../src/video/native_window.rs#L985)-[1048](../src/video/native_window.rs#L1048)
- mouse / wheel / drag: [src/video/native_window.rs:1049](../src/video/native_window.rs#L1049)-[1151](../src/video/native_window.rs#L1151)
- size / geometry event: [src/video/native_window.rs:1153](../src/video/native_window.rs#L1153)-[1214](../src/video/native_window.rs#L1214)

したがって owner thread を変えるときは、input semantics と IME context も ownership の一部として
移す必要がある。

## 3. repo の不変条件

### 3.1 必須 invariant

> `mimageviewer` 内で HWND を所有する thread は、message dispatch を無制限に遅らせる
> operation を実行してはならない。D3D11 / DXGI / DirectComposition、decoder、audio、
> VST bridge、file I/O、別 thread の完了を待つ operation は HWND owner thread に置かない。
> window lifecycle は GPU lifecycle の完了を同期的に待たず、世代付き request/ack で進める。

この invariant は「通常は短い」「API に timeout を渡した」「今の driver では再現しない」では
成立しない。外部 driver / plugin / OS callback の最悪時間に依存しない型・module・thread
ownership が必要である。

補助 invariant は次のとおり。

1. `mIVNativeVideoWindow` と HUD の create / mutate / pump / destroy は同じ pump thread が行う。
2. UI-owned main / detached viewport の create / pump / destroy は引き続き UI thread が行う。
3. render thread は HWND handle を target binding の opaque lease として受け取れるが、
   USER32 lifecycle API を呼べない module boundary に置く。
4. UI、pump、render のいずれも、別 thread の request 完了を同期的に待たない。
5. show / replace / destroy は request id と epoch で相関させ、古い ack と再利用された HWND を
   現在の target に適用しない。
6. placement 切替は「新 window hidden create → render target ready → show/publish →
   old window destroy」の順序を守る。render が止まれば old window を維持したまま cancel/close
   できる。
7. hidden presenter は visibility state であり、window の存在や pending field の有無を
   state sentinel にしない。
8. HWND owner の thread identity は runtime diagnostics と Windows test で表明する。

### 3.2 現行経路との対応

| invariant violation | §2 の経路 |
| --- | --- |
| presenter/HUD HWND owner が GPU wait を実行 | §2.1 の全 `mIVNativeVideoWindow` / HUD と §2.2 の全 GPU 経路 |
| pump の進捗が render loop 一周に従属 | [src/video/mod.rs:2353](../src/video/mod.rs#L2353)-[2370](../src/video/mod.rs#L2370) |
| window destruction が output loop termination を兼ねる | §2.3 の `post_quit_on_destroy` / `WM_QUIT` |
| placement replacement が window と GPU target を単一 thread / procedure で直列処理 | §2.1 の placement rebuild |
| implicit IME windows まで GPU wait thread に所属 | §2.1 の presenter IME 実測 |

`AcquireSync` の呼び出しを一箇所減らす、timeout を変える、destroy を遅延する、repaint を足す、
特定 placement だけ parent を外す、という変更はこの対応表の他の行を残すため、本 invariant の
修正ではない。

## 4. 選択肢

### 4.1 案 A: native video HWND を UI thread へ移す

#### A0. child HWND だけを UI thread 所有にする

`MainWindowChild` / `DetachedViewerChild` だけを UI thread で作れば、今回実測した
cross-thread child destruction は解消する。しかし `FullscreenBorderless` と HUD は引き続き
GPU wait thread が所有し、focus / activation / IME / close message が停止し得る。

- 影響範囲: child mode の create/destroy/resize と UI integration。
- 仕様への影響: child 入力が egui message handling と同じ thread に移る。
- 規模: Medium。
- 残るリスク: fullscreen/HUD/standalone popup の owner pump 停止、placement 間で異なる
  ownership rule、将来の同種再発。
- 判定: **症状パッチであり、採用不可**。実測された一経路だけを外し、破れた invariant を
  全 placement で回復しない。

#### A1. presenter と HUD の全 HWND を UI thread 所有にする

UI thread が child / popup / HUD の全 create/pump/destroy を行い、別 render thread が
GPU work を行う完全案である。これは全 HWND から GPU wait を除くため、構造的修正になり得る。

境界は次のようになる。

- UI は既存 `App::update` と winit message loop の中で window lifecycle request を処理する。
- render は世代付き HWND lease を受け、DirectComposition target / swapchain を構築する。
- render ready ack 前は window を表示しない。UI は ack を待たず次の frame/event loop へ戻る。
- `wnd_proc` の `WM_MOUSEACTIVATE`、key、mouse、IME preedit/commit、close event は UI thread
  上で走る。処理内容は維持するが、egui input と順序が競合しないよう一つの input router へ
  非同期 enqueue する。
- `post_quit_on_destroy` は廃止し、native output の typed lifecycle command で render を
  stop する。個別 HWND の `WM_DESTROY` は window epoch の終了通知だけにする。

trade-off:

| 観点 | 評価 |
| --- | --- |
| 影響範囲 | `App` / winit lifecycle、native wndproc、HUD、native presenter、placement、IME/input |
| 既存仕様 | UI thread 内で egui と native wndproc が同居する。message ordering、focus、IME composition、high-rate mouse/HUD hover の回帰面が大きい |
| UI 負荷 | wndproc 自体は軽く保てるが、region update、foreground claim、HUD hit-test 更新まで UI に流すと frame time を汚染する。全処理を enqueue-only に再整理する必要がある |
| 結合 | native video window lifecycle が `App` / eframe viewport lifecycle に強く結合し、detached rework 中の UI ownership model と同時変更になる |
| 実装規模 | Large |
| 残るリスク | UI thread 自体の長い egui frame が native input latency になる。winit の再入可能な callback と独自 HWND callback の順序、IME context の移動、UI thread の message queue 飽和 |
| 構造性 | **全 HWND を移す A1 は症状パッチではない**。ただし child だけの A0 は症状パッチ |

### 4.2 案 B: pump 専用 thread と render thread に分ける

新しい `native-video-window-pump` thread が全 `mIVNativeVideoWindow` と HUD を所有し、
`native-video-render` thread が全 GPU work を所有する。UI thread は host HWND と意図を
pump へ非同期送信するだけである。

#### thread boundary

| direction | 渡すもの | 渡さないもの |
| --- | --- | --- |
| App → pump | typed `Open` / `SwitchPlacement` / `SetVisibility` / `Close` / `Raise` / `Shutdown` request、placement、host HWND lease、request id | GPU frame、decoder/audio handle、同期 callback |
| pump → render | `AttachTarget` / `ResizeTarget` / `DetachTarget` / `SetPresentationMode`、window epoch、opaque HWND lease、pixel size | `NativeVideoWindow` / HUD RAII owner、USER32 mutation capability |
| render → pump | `TargetReady` / `TargetFailed` / `TargetDetached`、epoch、最新 HUD visual/hit-region snapshot、cursor/raise intent | GPU/DComp resource、blocking future、同期 response requirement |
| wndproc/pump → App or render | generation-stamped key/mouse/IME/close/resize event。bounded non-blocking send; coalesce 可能な mouse move/resize は latest-value | App lock、render lock、VST IPC wait |
| decoder → render | 現行どおり frame queue / source command | pump 経由の frame |

`Present`、keyed mutex、fence wait、DComp commit は必ず render thread が呼ぶ。Microsoft の
[DXGI multithreading guidance](https://learn.microsoft.com/en-us/windows/win32/direct3darticles/dxgi-best-practices)
が指摘するように、DXGI work と window thread を分けても render 側 API が window thread へ
message を送る可能性を考慮する必要がある。pump は常に dispatch 可能で、render はその応答を
待っても UI/pump を逆向きに待たない一方向依存にする。

DirectComposition の
[`CreateTargetForHwnd`](https://learn.microsoft.com/en-us/windows/win32/api/dcomp/nf-dcomp-idcompositiondesktopdevice-createtargetforhwnd)
は target HWND が同一 process であることを要求するが、別 thread で作成された HWND に対する
production driver 全組み合わせまで source/document だけで保証できない。そのため §6 の
test-only two-thread spike を production cutover の gate にする。

#### lifecycle

window state は複数の `bool` / `Option` / pending field で表さず、一つの closed enum に集約する。
名称は実装時に既存型と調整するが、意味は次を維持する。

```text
Empty
Preparing { request, staging, prior: NoPrior | Prior(host) }
Visible { request, host }
Hidden { request, host }
Switching { request, old, staging, visibility: Visible | Hidden }
Closing { request, hosts, reason }
Closed
```

`HostWindows` も `PresenterOnly` / `PresenterAndHud` の sum type とし、HUD の有無を別 flag に
しない。render 側も `NoTarget` / `Attaching` / `Ready` / `HiddenReady` / `Detaching` /
`Faulted` の一所有者で表す。request id / epoch が stale completion を reject する。

placement 切替:

1. pump が新 placement の HWND を hidden で生成し、new epoch の lease を render へ送る。
2. pump は message dispatch を継続する。旧 window は表示状態を保つ。
3. render が target create と first-frame prime を行い `TargetReady(epoch)` を返す。
4. pump が新 window を show/publish し、旧 window を hide/destroy する。
5. stale ready、failure、close は reducer が epoch 単位で処理し、古い HWND に触れない。

render が 3 で停止しても pump は close/shutdown を処理できる。安全でない Rust/GPU thread の
強制停止は行わず、その render session を quarantine して window と App の操作性を回復する。
driver call 自体が永続停止した場合、thread/GPU resource を process 終了前に回収できない可能性は
残るが、HWND owner と UI は巻き込まれない。

hidden presenter / 動画→音声 mode:

- pump は typed visibility command を受けて presenter/HUD を即時 hide する。
- render は既存の consume-and-hold behavior を継続し、window なしを source state の
  sentinel にしない。
- video へ戻るときは `TargetReady`/`PrimeReady` 後にのみ show する。
- source switch、前後 file、EOF continuous playback は render/source command であり、同じ
  placement の pump-owned HWND を不要に作り直さない。

`post_quit_on_destroy` の置換:

- `WM_DESTROY` は `Destroyed(epoch)` を reducer に通知するだけで、thread termination を
  表さない。
- pump loop の lifetime は typed state と明示 `Shutdown` command が所有する。
- close 中の全 pump-owned HWND の destroy が完了したときだけ最終 `WM_QUIT` を pump 自身へ
  post する。個別 window、placement switch、host destruction は pump thread を終わらせない。
- render stop は別 command であり、pump は stop ack / join を待たない。

wndproc は既存 semantics を保ちつつ、bounded/non-blocking の「decode and enqueue」に制限する。
foreground claim、window region rebuild、VST owner IPC、GPU HUD update など再入・block し得る
処理は dispatch 後の pump task にする。cross-thread window update は `PostMessage` または
`SWP_ASYNCWINDOWPOS` とし、`SendMessage` を新しい request/ack transport にしない。

trade-off:

| 観点 | 評価 |
| --- | --- |
| 影響範囲 | `src/video/mod.rs` の native output lifecycle、`native_window.rs`、`native_presenter` と HUD、App との command boundary、VST owner handoff |
| 既存仕様 | native video input/IME は独自 thread に残るため、egui UI thread との新たな message ordering 競合が A1 より少ない |
| UI 負荷 | UI は request enqueue と host lease publish のみ。GPU/HUD drawing は render、WndProc は pump |
| 実装規模 | Large。thread/channel/reducer 分離と DComp cross-thread validation が必要 |
| 残るリスク | DirectComposition/DXGI の thread/driver 差、channel backpressure、stale epoch、render hang 後の resource leak、pump command が wndproc に重い work を持ち込む退行 |
| 構造性 | **症状パッチではない**。child/popup/HUD/IME の全 owner thread から GPU wait を型と module 境界で除去する |

### 4.3 案 C: native presenter HWND を廃止して winit surface に統合する

winit/egui host の composition tree に video visual を直接統合し、別 child/popup HWND を
なくす案である。理論上は cross-thread child ownership 自体を消せるが、fullscreen、HUD、
zero-copy D3D11VA、wgpu/DirectComposition interop、detached/multi-viewport の surface lifetime
を同時に再設計する必要がある。

- 影響範囲 / 規模: Very Large。renderer と detached architecture の双方を再設計する。
- 利点: native child HWND の focus/z-order/IME 問題を減らせる可能性がある。
- 残るリスク: zero-copy/performance regression、wgpu device loss、fullscreen/HUD integration、
  release/driver matrix。
- 判定: 長期候補としては構造的だが、P1 hard hang の修正として scope と検証面が大きすぎる。

## 5. 推奨案

**案 B を推奨する。** A1 も invariant を満たせるが、video 固有の wndproc、IME、HUD の高頻度
input を UI/egui thread へ持ち込み、detached rework 中の `App` / viewport ownership と変更面を
重ねる。案 B は native window semantics を専用 pump に保ったまま、危険な GPU wait だけを
別 owner に切り離す。全 placement と HUD に同じ rule を適用でき、render hang 時にも main /
detached viewer の destroy、activation、IME dispatch を進められる。

ただし「thread を二本にする」だけでは不十分である。pump が render ack を同期 wait したり、
wndproc から GPU/VST IPC を呼べば同じ invariant を別形で破る。typed reducer、一方向の非同期
request/ack、module-private HWND owner、stall test を推奨案の不可分な条件とする。

## 6. 段階実装計画

全 7 段階とする。Stage 1〜3 は production behavior を変えず、Stage 4 を全 placement 一括の
atomic cutover にする。child だけを先に切り替えた混在状態を production fallback として残さない。
各段階は独立した commit/test gate を持ち、停止時はその段階で有効な ownership model が一つに
定まる。

### Stage 1: contract、reducer、観測点を先に固定

変わるもの:

- production window を操作しない pure `WindowHostState` / command / event / epoch contract と
  fake backend test を追加する。
- placement → style/parent-owner/HUD の既存 mapping を exhaustive test にする。
- 現行 thread id、HWND generation、pump/render progress を記録する diagnostics schema を定義する。
- architecture decision と Windows test harness の timeout/error vocabulary を用意する。

変わらないもの:

- production は現在の一 thread presenter のまま。
- window create/pump/render、input、IME、VST、hidden/EOF behavior は一切変わらない。

単独 gate: reducer/property tests、mapping tests、diagnostics serialization test。

#### Stage 1 実装記録 (2026-07-29)

実装 file は `src/video/window_host_contract.rs` と、既存 mapping を保持したまま exhaustive
テストを置いた `src/video/mod.rs` である。production の presenter command / event channel、
HWND、thread spawn には接続していない。

実装した contract / state 型:

- identity / lease: `WindowRequestId`、`WindowEpoch`、`OpaqueWindowId`、
  `WindowGeneration`、`OpaqueWindowHandle`、`WindowLease`。HWND の数値再利用だけでは
  current target と一致せず、request + epoch + HWND generation で相関する。
- ownership: `HostWindowTopology`、`HostWindows` (`PresenterOnly` /
  `PresenterAndHud`)、`WindowHostSpec`、`HostedWindow`、`StagingWindow`、`PriorHost`、
  `ClosingHosts`。HUD の有無や staging/closing を bool / `Option` field に分解しない。
- reducer: `WindowHostState` (`Empty` / `Preparing` / `Visible` / `Hidden` /
  `Switching` / `Closing` / `Closed`)、`WindowHostCommand`、`WindowHostEvent`、
  `WindowHostEffect`、`WindowHostTransitionStatus`、`reduce_window_host`。window create は
  hidden effect に限定し、`TargetReady` 前 publish 禁止、stale request/epoch reject、render
  ack 不要 close を pure transition で表す。
- diagnostics: schema version 1 の `NativeWindowDiagnostics`、`DiagnosticThread`、
  `PumpDiagnostics`、`RenderDiagnostics`、`ProgressStamp`、`RenderOperation`。pump/render
  thread id、source generation、host state と HWND generation、pump message/command、render
  operation の開始・完了時刻を serialize できる。
- Windows harness 語彙: `WindowsHarnessPhase`、`WindowsHarnessTimeout`、
  `WindowsHarnessInvariant`、`WindowsHarnessError`。Stage 3 の bounded wait / backend error /
  disconnect / invariant violation を同じ表現で扱う。

追加したテスト:

- `fake_backend_does_not_publish_before_target_ready`
- `fake_backend_switch_stall_keeps_old_window_and_close_needs_no_render_ack`
- `stale_epoch_property_rejects_ready_even_when_raw_hwnd_is_reused`
- `close_property_reaches_closed_from_every_nonterminal_state_without_target_ready`
- `idempotency_property_duplicate_request_does_not_repeat_effects`
- `close_cancels_staging_with_the_original_request_identity`
- `diagnostics_schema_serializes_thread_hwnd_generation_and_progress`
- `windows_harness_timeout_and_error_vocabulary_is_stable`
- `native_video_placement_mapping_is_exhaustive` (`NativeVideoPlacement` 4 variant ×
  mode / owner / HUD の 3 軸 = 12 mapping。HUD request-off / env-disabled も各 placement で確認)

Stage 1 の意図的な未達 / 後続:

- contract、diagnostics、harness 語彙は production 未配線。現行一 thread presenter と runtime
  thread 数は変更していない。
- `NativeWindowHost` / `NativeRenderCore` の実体分離、window owner の `!Send`、生成 thread
  assertion、HUD owner の GPU core からの分離は Stage 2。
- disposable HWND / DirectComposition の two-thread 実行 harness と実 timeout は Stage 3。
- production pump/render channel、watchdog/ping、health log、入力/IME/VST/source/EOF の完全な
  sequence test は各 Stage 4〜7。Stage 1 の fake backend は実 HWND / GPU / driver を検証しない。

### Stage 2: window owner と render core を module/type で分離

変わるもの:

- `NativeVideoWindow` / HUD を `NativeWindowHost`、D3D/DXGI/DComp を `NativeRenderCore` に
  分ける。
- window-owning type を `!Send` にし、creation thread id assertion を持たせる。
- render module へ HWND RAII owner と USER32 mutation API を公開しない。
- HUD HWND ownership を `NativeVideoPresenter` GPU core から外す。

変わらないもの:

- 両 component はまだ現行 `native-video-presenter` thread 上で直列実行する。
- runtime thread 数、placement lifecycle、描画、input、IME、VST の挙動は変えない。

単独 gate: compile-fail/thread-affinity tests、既存 presenter tests、`cargo check`。この時点では
P1 は未修正だが production regression はない。

#### Stage 2 実装結果 (2026-07-29)

実際の分離:

- `src/video/native_window_host.rs` の `NativeWindowHost` が presenter/HUD の HWND RAII owner、
  visibility、placement geometry、region、z-order、focus/IME/cursor mutation を所有する。
  HUD の実装は `src/video/native_window_host/hud_window.rs` へ移し、
  `PresenterOnly` / `PresenterAndHud` の sum type で topology を一つに集約した。
- `src/video/native_presenter/render_core.rs` の `NativeRenderCore` が D3D11/DXGI/DComp、swap chain、
  GPU present と overlay 描画を所有する。`native_presenter/mod.rs` は facade と
  `overlay_draw` の module 宣言だけを残す。
- 両型の生成・呼び出しは `run_native_video_output` と DComp presenter test の現行
  `native-video-presenter` thread 上で直列のまま。production thread 数と lifecycle は変えていない。

型/module 境界:

- `NativeWindowHost`、`NativeVideoWindow`、`HudOverlayWindow` は
  `PhantomData<Rc<()>>` で `!Send + !Sync`。各 owner は creation `ThreadId` を保持し、
  HWND access/mutation と `Drop` で owner-thread assertion を通す。
- render config へ渡すのは private HWND field を持つ非所有 `NativeRenderTargets` lease だけ。
  lease は DComp target binding と read-only query に限定し、raw HWND、RAII owner、
  `ShowWindow` / `SetWindowPos` / `DestroyWindow` / focus / IME / cursor mutation を公開しない。
- focus/IME/cursor は `NativeWindowIntent`、HUD hit-region は値 snapshot として render から返し、
  同じ loop 反復で host が適用する。render source へ mutation API が再流入していないことは
  source-boundary assertion test で固定した。

compile-fail gate は `trybuild` を追加せず、assertion test 形式を採用した。
`NativeWindowHost` と下位 owner の negative trait assertion、private target field、render source の
forbidden-capability assertion を組み合わせる。creation-thread 外操作は affinity assertion の panic を
別 thread から捕捉する unit test で固定する。

分離時に確認した、まだ境界を跨ぐ現行箇所 (Stage 4/5 見積もり。Stage 2 では挙動を変えない):

1. `run_native_video_output` は wndproc event drain、host mutation、render call を一つの loop で直列実行し、
   pump/render channel、epoch 付き attach/detach、reducer は未接続。
2. opaque target lease 経由の DComp attach に加え、DPI、cursor client 座標、focus/foreground の
   synchronous read は render 起点のまま。Stage 4/5 では pump event/snapshot と intent に寄せる。
3. main-window child resize subclass は global child HWND publish と `SWP_ASYNCWINDOWPOS` を使う既存経路のまま。
   専用 pump cutover 時に host registry/request へ統合する必要がある。
4. `hwnd_out` / `hud_hwnd_out` による App/VST への HWND publish と VST owner の
   fire-and-forget handoff は現行のまま。owner-applied ack/anchor は Stage 5。
5. per-window `post_quit_on_destroy` / stale `WM_QUIT` discard は現行 lifecycle のまま。
   typed pump shutdown への置換は Stage 4。

Stage 3 へ持ち越すもの:

- disposable pump thread と別 render thread の実 HWND/DComp attach-present-detach harness。
- deliberate render stall 中の pump ping/resize/close/parent destroy の bounded-time gate。
- GPU/driver、HWND reuse、DPI/monitor、DComp target recreate matrix の実機記録。

P1 hang、production pump/render 分離、channel/reducer、VST owner ack はこの Stage 2 では未修正。

### Stage 3: test-only two-thread spike

変わるもの:

- disposable test HWND を専用 pump thread が作り、別 render thread がその HWND に
  DirectComposition target/swapchain を attach/present/detach する Windows-only harness を作る。
- render を意図的に停止したまま pump ping、resize、close、parent destroy を実行する。
- NVIDIA/Intel/AMD、DComp target recreate、HWND reuse、DPI/monitor change を実機 smoke の
  gate として記録する。

変わらないもの:

- production runtime は現行 ownership のまま。ユーザー設定/profile、実動画 pipeline を使わない。

単独 gate: test process が bounded time で終了し、pump ping/close が render stall から独立する。
DirectComposition の cross-thread target が対象環境で成立しなければ Stage 4 へ進まず、A1 を
再評価する。

#### Stage 3 実測結果 (2026-07-29)

**結論: 対象の NVIDIA 環境では成立した。** pump 専用 thread が所有する別 thread の child
HWND に対し、render thread から DirectComposition target と composition swap chain を
attach し、色 clear、`Present`、`Commit`、`WaitForCommitCompletion`、`DwmFlush` まで成功した。
render thread を DComp resource 保持中の channel wait で意図的に停止しても、pump の ping、
resize、close、別 thread 所有 child を持つ parent の destroy はすべて bounded time で完了した。
停止解除後の `SetRoot(None)`、commit completion、resource drop も成功したため、この実測環境では
案 B の Stage 4 前提を満たす。production runtime、設定/profile、decoder、実動画は使用していない。

Windows-only harness は `src/video/native_window_thread_spike.rs` に test-only で置き、
`src/video/mod.rs` から `cfg(all(test, windows))` のときだけ接続した。通常 CI / 通常の
`cargo test --lib` では hardware gate を走らせないよう `#[ignore]` とし、明示実行時は外側 test が
同じ test binary を subprocess として起動する。外側 watchdog は 30 秒で subprocess を kill して
失敗にするため、Win32 / driver call が永久停止しても test runner を無限に止めない。個別 phase は
3 秒、GPU attach/present/detach は 10 秒で待つ。成功経路では close / parent destroy 後の pump
barrier、全 HWND の `IsWindow == false`、3 thread の終了を確認した。

実行コマンドと結果:

```powershell
cargo test -p mimageviewer --lib video::native_window_thread_spike::cross_thread_dcomp_present_remains_pump_independent_when_render_stalls -- --ignored --exact --nocapture
# 1 passed, child 実測 0.37 s / watchdog 込み 0.47 s

cargo test -p mimageviewer --lib
# 4454 passed, 0 failed, 19 ignored
```

最終実測値 (同一 run):

| 検証項目 | 結果 | 実測 / 完了境界 |
| --- | --- | --- |
| HWND owner 分離 | 成立 | parent owner thread、pump thread、render thread がすべて別 Win32 thread ID。2 child の owner が pump thread と一致 |
| cross-thread DComp attach / present | 成立 | case 1: attach 2.709 ms、present + commit completion + `DwmFlush` 26.436 ms。case 2: 1.158 ms / 27.731 ms |
| render stall 中 pump ping | 成立 | case 1: 0.032 ms、case 2: 0.033 ms |
| render stall 中 resize | 成立 | child client 320x180 → 384x216、0.672 ms |
| render stall 中 close | 成立 | pump thread の `WM_CLOSE` → `DestroyWindow` 復帰まで 5.003 ms。復帰後 `IsWindow == false` |
| render stall 中 parent destroy | 成立 | parent owner thread の `DestroyWindow(parent)` 復帰 4.349 ms、child `WM_NCDESTROY` 後の pump barrier まで 5.003 ms。parent / child とも `IsWindow == false` |
| HWND 破棄後の DComp detach | 成立 | case 1: 26.329 ms、case 2: 24.432 ms。各 `SetRoot(None)` + commit completion + `DwmFlush` 成功 |
| test process bounded exit | 成立 | 30 秒 watchdog 内に subprocess が正常終了。全 4 HWND と全 3 worker thread の cleanup を確認 |
| composed pixel の画素 / 目視検証 | 未検証 | API-level の clear / `Present` / commit completion / `DwmFlush` は成功したが、screen capture / pixel probe は実施していない |

実行環境:

- OS: registry `ProductName = Windows 10 Pro`、`DisplayVersion = 25H2`、build
  `26200.8875`、`BuildLabEx = 26100.1.amd64fre.ge_release.240331-1435`。
- DXGI adapter: NVIDIA GeForce RTX 4090、vendor `0x10DE`、device `0x2684`、dedicated VRAM
  24138 MiB、D3D feature level `0xB100` (11.1)。
- display driver: `32.0.15.9621`。`IDXGIAdapter::CheckInterfaceSupport(ID3D11Device)` では version
  を取得できなかったため、同じ adapter 名 / PCI vendor+device に一致する read-only の
  `HKLM\\SYSTEM\\CurrentControlSet\\Control\\Video\\...\\0000` から採取した。

未検証のまま残る matrix:

| Matrix | 状態 | 未知の内容 / 後続 gate |
| --- | --- | --- |
| NVIDIA の別世代 / 別 driver | 未検証 | RTX 4090 + 32.0.15.9621 の 1 点のみ。driver update / downgrade 差は未知 |
| Intel | 未検証 | machine には Intel Graphics driver も存在するが、この run の hardware D3D device は RTX 4090。Intel adapter 強制選択は未実施 |
| AMD | 未検証 | adapter / driver とも未実施 |
| hybrid GPU / adapter 切替 / RDP / WARP | 未検証 | process 中の adapter change、remote session、software adapter は未実施 |
| DComp target recreate | 部分検証 | 同じ D3D / DComp device で異なる 2 child HWND へ順次 target を作る経路は成功。同一 HWND への detach → target recreate、failure 後 recreate は未検証 |
| HWND 数値 reuse | 未検証 | 2 HWND は別値。破棄後に同じ raw HWND 値が再利用される case は未発生 |
| DPI / monitor change | 未検証 | per-monitor DPI change、monitor 間移動、refresh-rate / HDR / MPO 差は未実施 |
| visible composed pixels | 未検証 | API submission / completion の成立のみ。visible window での画素、ちらつき、色、present cadence は Stage 7 実機 gate に残す |
| 長時間 / 繰り返し / device lost | 未検証 | 2 lifecycle のみ。大量 recreate、TDR、device removed、DWM restart は未実施 |

設計判断は **Stage 4 へ進んでよい** とする。ただしこれは上記 RTX 4090 環境で案 B の中核前提を
否定する結果が出なかったという判断であり、全 driver matrix の保証ではない。Stage 4〜7 でも
GPU vendor / target recreate / HWND reuse / DPI・monitor の未検証欄を gate として保持し、別環境で
`CreateTargetForHwnd`、`Present`、commit completion、detach、または parent destroy の不成立が
再現した場合は production fallback を追加せず、Stage 4 cutover を止めて A1 へ設計判断を戻す。

### Stage 4: 全 placement と HUD を専用 pump へ atomic cutover

変わるもの:

- `native-video-window-pump` を起動し、全 placement の presenter/HUD create/mutate/pump/destroy
  を移す。
- `native-video-render` は GPU resource/present のみを持つ。
- §4.2 の request/ack と二相 placement switch を production に接続する。
- `post_quit_on_destroy` / per-window `WM_QUIT` を typed pump shutdown に置き換える。
- 現行 wndproc の mouse/key/IME/close semantics と VST owner selection は同等に維持し、
  non-blocking event route に載せる。
- close は render ack/join を待たず HWND を閉じられる。render fault は session quarantine
  として扱う。

変わらないもの:

- `NativeVideoPlacement` の意味、main/detached host registry、detached viewport ownership、
  fullscreen/in-window/detached の見た目、HUD操作、hidden behavior、VST owner policy。
- decoder/audio threads と frame queue ownership。

単独 gate: placement matrix、parent-destroy-under-render-stall test、input/IME event routing test、
portable smoke。ここで初めて P1 の ownership invariant が production で成立する。

### Stage 5: VST owner handoff と focus/IME 境界を堅牢化

変わるもの:

- VST bridge の `set_chain_owner` に request id と owner-applied ack を追加する。
- owner 切替中は pump-owned hidden owner anchor を保持し、ack 後に旧 presenter HWND を
  destroy する。UI/pump は ack を同期 wait しない。
- bridge GUI task timeout 時は既存の bridge isolation/termination policy へ収束させ、main /
  pump を待たせない。
- `WM_MOUSEACTIVATE`、foreground claim、IME preedit/commit、HUD focus return の sequence test
  を追加する。

変わらないもの:

- plugin DSP/audio path、editor の見た目、fullscreen 時のみ presenter を editor owner にする
  policy。Stage 4 時点の fire-and-forget behavior を劣化させず、ack 付きへ強化する。

単独 gate: fake bridge ack/stall/restart tests、focus/IME handler tests、VST editor owner
Windows test。

### Stage 6: hidden/source/EOF と placement failure の lifecycle hardening

変わるもの:

- 動画→音声→動画、前後 file、EOF continuous playback、pause 中 placement switch を reducer
  sequence test で全列挙する。
- render prime failure / device loss / stale ready / close during switch を一つの typed transition
  で処理する。
- §1.25/§1.26 の将来 `FramePresentationState` と接続できる interface を固定するが、本 P1
  のために GPU frame state の個別 workaround を追加しない。

変わらないもの:

- source selection、EOF policy、audio continuity、hidden consume-and-hold のユーザー仕様。
- §1.25/§1.26 自体の実装範囲。

単独 gate: pure sequence/property tests、fake delayed render tests、source/placement integration
tests。

### Stage 7: legacy path 除去、health detection、最終実機 gate

変わるもの:

- 一 thread の window-owning presenter path と `post_quit_on_destroy` compatibility code を削除する。
- HWND owner assertion と §7.3 の health watchdog/log を常時有効にする。
- `video-architecture.md`、detached plan §11、必要な release notes を更新する。
- full automated gate と最小の実機確認を行う。

変わらないもの:

- user-facing placement/input/playback specification。legacy fallback を残して ownership rule を
  二重化しない。

単独 gate: compile/tests/fmt/glyph check、portable smoke、§7.2 の 5 シナリオ。

## 7. テスト戦略

### 7.1 自動テストで固定する invariant

pure / state-transition:

1. 全 `NativeVideoPlacement` から style、parent/owner、HUD 構成への exhaustive mapping。
2. host reducer の合法遷移、全 state からの close/shutdown、同じ request の idempotency。
3. stale request/epoch/ack を無視し、HWND 値再利用だけでは current target と判定しない。
4. `TargetReady` 前に staging window を show/publish しない。
5. switch 中に render が止まっても old window を維持し、close は render ack なしで完了する。
6. `Visible` / `Hidden` / source switch / EOF / pause の sequence。window existence や
   `Option` を mode sentinel にしない。
7. presenter+HUD の paired create/destroy と、fullscreen 以外で HUD が存在しないこと。
8. VST owner anchor の old owner → request → ack → old destroy 順序と timeout/restart。
9. key/mouse/IME/close event の epoch routing、stale window 由来 event の reject。
10. channel full 時に mouse move/resize は coalesce し、close/key/IME commit は lossless bounded
    queue または明示 overflow fault へ進むこと。

type/module:

11. `NativeWindowHost` / HUD owner が `!Send` である compile-fail test。
12. render core が `DestroyWindow` / `ShowWindow` / HWND subclass mutation capability を取得できない
    visibility boundary。
13. runtime assertion で `GetWindowThreadProcessId` が presenter/HUD は pump thread、main/detached
    host は UI thread と一致すること。

Windows integration:

14. UI test thread が parent、pump thread が child、render thread が deliberate stall という
    harness で、parent close/destroy と pump ping が例えば 2 秒以内に完了すること。timeout は
    production recovery ではなく regression detector として使う。
15. render stall 中にも presenter/HUD の posted ping、resize、close が dispatch されること。
16. cross-thread DComp attach/present/detach、placement recreate、HWND reuse、device-loss injection。
17. render ready/failure を遅延・逆順に返す fake backend で、二相 switch と shutdown が
    deadlock しないこと。

### 7.2 実機確認が必要な項目

unit/integration test だけでは実 driver の内部 stall、D3D11VA zero-copy、MPO/DWM、
multi-monitor DPI、Windows TSF/日本語 IME、第三者 VST editor の callback/z-order を保証できない。
実機確認は次の **5 項目**に絞る。各項目は正常性だけでなく、操作後も main UI / tray / close が
応答することを確認する。

1. **元ハングの最小確認**: hardware decode 動画を in-window と detached で開き、先頭 frame
   表示直後に host を閉じる操作を各 10 回。アプリが閉じ/切替を完了し、
   `NATIVE VIDEO WINDOW PUMP STALL` と `UI THREAD HANG` が出ない。
2. **placement / multi-window**: 通常 fullscreen → in-window → detached/F12 → fullscreen を
   往復し、別 monitor への移動・resize・複数 viewer を確認する。black window、旧 window 残留、
   z-order/focus、DPI、first-frame flash の回帰がない。
3. **hidden presenter / source continuity**: video → audio file → 前/次 file → video、ならびに
   video EOF 連続再生を行う。audio 中に window/HUD が出ず、video 復帰時の frame/audio position
   と連続再生が維持される。
4. **HUD / mouse / keyboard / IME**: fullscreen HUD の seek bar/buttons、click/wheel/drag、
   cursor auto-hide、Alt+Tab/Escape、bookmark 等の日本語 preedit/commit を確認する。二重入力、
   composition window の誤表示、focus stealing がない。
5. **VST GUI**: editor 表示中に fullscreen/in-window/detached 切替、video→audio、host close を
   行う。editor の owner/z-order/focus、plugin child、bridge process が正しく追随し、終了時に
   main UI が待たない。

実機 gate では agent が通常 profile binary を起動せず、repo の verification build handoff
規約に従う。hardware/real settings が必要ならユーザーが normal-profile core を起動する。

### 7.3 再発の検知

既存の `panic.log` `UI THREAD HANG suspected` は UI 側の結果しか示さない。原因 thread を同じ
incident で判別できるよう、lock-free/latest-value の `NativeWindowHealth` を追加する。

- pump thread id、presenter/HUD HWND と epoch、最後に dispatch した message sequence/time。
- pump command の last received/completed request id と time。
- render の last started/completed operation
  (`Attach` / `AcquireSync` / `FenceWait` / `Present` / `DCompCommit` / `Detach`) と epoch/time。
- placement、source generation、visibility state。path や media metadata は記録しない。
- watchdog は pump へ generation/sequence 付き posted ping を送り、ack sequence を atomic に
  観測する。watchdog thread 自身も `SendMessage` や renderer lock を使わない。

判定:

1. HWND alive かつ pump ping 未応答が閾値を超えたら
   `NATIVE VIDEO WINDOW PUMP STALL`。
2. pump は応答し render operation だけが閾値を超えたら
   `NATIVE VIDEO RENDER STALL`。UIを止めず、session quarantine/close が可能かも記録する。
3. `UI THREAD HANG` 発生時は同じ timestamp の pump ack age / render operation age / epoch を
   一行に含め、相互待ちか UI 固有 stall かを区別する。
4. transition edge と 10 秒 rate limit で記録し、busy polling や log flood を避ける。
5. debug/test build では window publish 時に `GetWindowThreadProcessId` assertion を行い、
   production では不一致を一度だけ重大 event として記録する。

これにより今回の再発は「UI hang が続いた」だけでなく、pump も止まったのか、render だけが
driver 内で止まったのか、どの HWND generation/operation だったかまで検知できる。

## 8. 既存機能との相互作用

| 機能 | 現行 interaction | 壊してはいけない挙動 / 設計上の処置 |
| --- | --- | --- |
| 通常 fullscreen playback | borderless popup は presenter thread 所有、owner は main。HUD popup を伴う | popup と HUD を同じ pump が所有。first target ready 後に show。main の focus/activation、cloak/foreground、cursor hide、exclusive でない borderless semantics を維持 |
| in-window playback | `WS_CHILD` presenter、parent は main。main resize subclass から async position update | pump-owned child の parent は引き続き main。UI は子の応答を待たず、resize は async/coalesced。clip/region、aspect、mouse routing を維持 |
| detached viewer / F12 | `DetachedViewerChild` の parent は UI-owned detached host。host HWND は registry から解決 | registry/viewport owner を変更しない。host generation change は typed switch request。host close 時は pump が render を待たず child を処理でき、geometry を失っただけで viewport を作り直さない |
| 複数 window mode | 各 egui viewport は UI thread。active video output だけ native presenter を持つ現在の resource policy | context-scoped request/epoch とし、別 viewer の host/queue/cache を invalidate しない。passive/static viewport は現状維持 |
| placement 切替 | 同一 presenter thread が new HWND/new GPU target を作り prime 後に old destroy | 二相 switch で見た目の順序を維持。stale ack、close-in-switch、prime failure で旧 window を誤破棄しない |
| hidden presenter (video→audio) | window を hide し GPU frame を consume-and-hold。再表示可能性を維持 | pump visibility と render presentation state を別 typed owner にする。audio mode を HWND absence で表さず、HUD も同時 hide。video 復帰は prime ready 後 |
| VST GUI | fullscreen 時は editor owner を presenter、それ以外は main に切替 | request/ack/owner anchor で旧 owner HWND を ack 前に destroy しない。plugin GUI thread/bridge を UI/pump から同期 wait しない |
| HUD 操作 | presenter-owned DComp HUD + popup hit window。seek/button/mouse event を送る | HUD HWND は pump、HUD texture/composition は render。hit-region snapshot は epoch 付き latest-value。seek/button edge event を落とさず、mouse move は coalesce 可 |
| mouse / keyboard | native wndproc が mouse/key/escape/activation を event channel へ送る | handler semantics を維持。WndProc 内は enqueue-only。close_on_escape policy、foreground 判定、Alt+Tab、wheel/drag を回帰 test |
| IME | presenter wndproc が preedit/commit と default composition window suppression を処理 | input HWND と暗黙 TSF/IME window は pump thread へ移る。同じ context で preedit/commit を処理し、UI thread との二重 commit を防ぐ |
| video→audio 前後 file | hidden presenter を保持し source/audio を切替 | window lifecycle と source lifecycle を結合し直さない。前/次で不要な destroy/recreate をせず、復帰可能性と audio continuity を維持 |
| EOF 連続再生 | source/output loop が次項目へ進み frame を再供給 | EOF を window close と解釈しない。source generation と window epoch を別に保ち、同 placement なら host を再利用 |

## 9. 凍結ルールへの適合

### 9.1 detached rework §2 との照合

本設計は [detached-rework-plan.md §2](detached-rework-plan.md#2-リワーク中の凍結ルール)
を次のように満たす。

- **矩形・focus の heuristic を追加しない**: detached host は既存 registry / runtime owner から
  渡される HWND lease を使う。screen rect、`context_menu_open`、一定時間 window を根拠に
  host を推測しない。
- **geometry loss で viewport を recreate しない**: placement request が明示した host generation
  の変更だけを扱う。host rect 不明、resize 遅延、render ready 遅延を detached 再生成理由にしない。
- **App-global bool / Option / pending field を増やさない**: native output context 内の一つの
  closed enum reducer が ownership/lifecycle を表す。detached 固有 flag を足さない。
- **既存 placement owner を重複させない**: `ViewerPresentation` /
  `NativeVideoPlacement` と detached host registry が決めた intent を消費し、別の placement
  store や geometry-based owner を新設しない。
- **context scope を保つ**: request/epoch/channel/window/GPU resource は一つの native output
  session に属し、別 detached/static viewer の open/close で mutation-style reset を配信しない。
- **症状回避をしない**: destroy delay、retry、repaint、timeout 延長、child だけの owner 変更、
  `AcquireSync` 禁止箇所の追加ではなく、全 presenter/HUD HWND owner から GPU wait capability を
  取り除く。

したがって本設計が触れる detached path は「既存 host HWND を世代付き request として
pump に渡す」境界に限られる。detached viewport の表示判定、矩形、focus、再生成、navigation
state の仕様変更は行わない。

### 9.2 症状パッチではない根拠

root cause は特定の `AcquireSync` call ではなく、`run_native_video_output` が
`HWND owner + message pump + unbounded GPU work` を同一 thread に束ねる ownership である。
案 B はその組を型と thread boundary で成立不能にする。

- 2026-07-29 に実測した child destroy だけでなく、fullscreen owned popup、standalone popup、
  HUD、IME/focus message に同じ invariant を適用する。
- GPU API の種類や timeout に依存せず、将来の present/fence/DComp wait も render thread に
  閉じる。
- lifecycle termination を `WM_DESTROY` の副作用から typed owner へ移し、placement/hidden/
  failure/close を同じ state machine で扱う。
- deliberate render stall の自動 test が「pump/parent destroy は進む」を直接検証する。

このため、revert 済みの `rearm_presented_shared_output` を戻す/避けることとも、§1.25 の暫定
`AcquireSync` 運用制約とも独立した構造修正である。

### 9.3 実装時の §11 記録草案

設計だけの現時点では §11 に実装済みとして記録しない。Stage 4 の cutover 前に ClaudeCode と
Codex が §2 適合を再確認し、実装 commit と実測結果を用いて次の内容を追記する。

| 日付 | 変更 | 触った範囲 | 凍結ルール適合理由 | 検証 |
| --- | --- | --- | --- | --- |
| YYYY-MM-DD | native video HWND pump と GPU render thread を分離。全 placement/HUD の create/pump/destroy を専用 pump 所有へ移し、`post_quit_on_destroy` を typed lifecycle に置換 | `src/video/mod.rs`、`src/video/native_window.rs`、`src/video/native_presenter/*`、App→native output command/host lease boundary。detached viewport/rect/focus model 自体は変更しない | root cause である「HWND owner が unbounded GPU wait を実行」を全 placement で不可能にする。rect heuristic、viewport recreate、App-global detached flag、delay/retry を追加せず、既存 host registry と placement owner を read-only 入力として使うため症状パッチではない | reducer/type tests、render-stall parent-destroy Windows test、全 placement/hidden/EOF/VST tests、portable smoke、実機5項目 |

記録には実際に変更した file、Stage、commit、未解決 risk、DirectComposition cross-thread spike の
対象 GPU/Windows build も添える。

## 10. 残る risk と実装前 gate

1. **DirectComposition / driver 差**: same-process 別 thread HWND への target attach が document 上
   十分明示されないため、Stage 3 を必須 gate とする。不成立なら A1 へ設計判断を戻す。
2. **render thread の永続停止**: Rust/driver call を安全に強制終了できない。pump/UI 可用性は
   回復できても、resource/thread は process 終了まで残り得る。再利用せず session quarantine する。
3. **channel overload**: high-rate mouse/resize/HUD update は latest-value/coalesce、close/key/IME
   commit は lossless bounded path に分類し、unbounded queue と silent drop の両方を避ける。
4. **cross-thread synchronous Win32 call の再流入**: module API review、compile boundary、
   stall test で防ぐ。DXGI から pump への OS 内部 message は許容するが、pump→render の逆向き wait を作らない。
5. **IME context 移動**: presenter input HWND の owner thread が変わるため、実日本語 IME 確認は
   省略できない。
6. **VST 第三者 code**: bridge process で隔離されていても editor GUI thread は停止し得る。
   owner ack/anchor と既存 timeout/bridge termination で main/pump の wait へ波及させない。

## 11. 調査中に判明した隣接問題

VST owner 切替は現状 fire-and-forget である。Rust は
`set_chain_owner` を bridge へ送り
([src/video/dsp/bridge.rs:584](../src/video/dsp/bridge.rs#L584)-[590](../src/video/dsp/bridge.rs#L590))、
bridge は slot GUI thread へ async post して `GWLP_HWNDPARENT` を変更する
([crates/vst3-host/src/plugin_loader.cpp:1794](../crates/vst3-host/src/plugin_loader.cpp#L1794)-
[1832](../crates/vst3-host/src/plugin_loader.cpp#L1832))。Rust 側には「owner 変更が実際に適用された」
ack がない。

そのため placement 切替で旧 presenter HWND を破棄するとき、VST GUI thread が遅延/停止していれば
editor popup が旧 HWND を owner として保持したままになる可能性がある。今回実測された deadlock の
直接原因ではないが、HWND lifecycle 分離時に無視できない cross-thread ownership race である。
Stage 5 の request id 付き ack と pump-owned owner anchor で同時に閉じる。UI/pump が VST ack を
同期 wait する修正は採用しない。

もう一つの実装 gate は、DirectComposition が別 thread 所有 HWND を対象にする driver matrix で
ある。これは既知 bug と断定せず、Stage 3 で production cutover 前に検証すべき未確定 risk として扱う。
