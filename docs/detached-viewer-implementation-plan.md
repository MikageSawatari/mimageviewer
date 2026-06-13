# 画像・動画ビューア別ウィンドウ実装計画

v1.4.0 後に着手する「画像・動画をメイン一覧とは別ウィンドウで表示する」機能の実装計画。
元の要望と方針整理は [next-release-backlog.md §4.3](next-release-backlog.md#43-画像動画ビューアの別ウィンドウ化) を正とする。

## 1. 目的

- 画像・動画を同じ操作モデルで別ウィンドウ表示できるようにする。
- 別ウィンドウが開いている間は、メイン一覧のカーソルとビューアの表示対象を常に同期する。
- 動画だけ従来挙動になる状態は避ける。動画も初回リリース範囲に含める。
- 別ウィンドウからのフルスクリーン化は初期仕様に含めない。
- 複数の独立ビューアウィンドウは対象外。常に 1 セッション、1 表示対象。

## 2. ユーザー向け仕様

### 2.1 表示モード

- `F12`: 別ウィンドウモード ON/OFF を切り替える。
- 別ウィンドウモード ON 中に画像・動画を開くと、別ウィンドウで表示する。
- 別ウィンドウモード OFF 中は、従来の同一ウィンドウ / フルスクリーン系の表示を使う。
- 再生中・表示中に `F12` を押した場合は、可能な限り再生位置・再生状態・現在ページを維持したまま表示先だけ切り替える。
- 別ウィンドウモード中の `F11` は無効にする。必要なら短い通知だけ出し、フルスクリーン化はしない。

### 2.2 セッション開始と終了

- 別ウィンドウモード ON でも、別ウィンドウが閉じているだけならカーソル移動では再表示しない。
- `Enter`、ダブルクリック、既存の「開く」操作で初めて別ウィンドウを開く。
- 別ウィンドウの `×` は、別ウィンドウモード OFF ではなく「現在のビューアセッション終了」とする。
- 別ウィンドウの `×`、`Esc`、`Enter`、右クリック、`Alt+F4` はすべて同じ終了操作に寄せる。
  - 画像: 表示セッションを終了する。
  - 動画: 再生を停止し、動画セッションを終了する。
- セッション終了後も別ウィンドウモード設定は維持する。次に画像・動画を開いたときに再表示する。

### 2.3 メイン一覧との同期

- 別ウィンドウが開いている間、メイン一覧の `selected` が表示可能な画像・動画へ変わったら、ビューアも同じ項目へ切り替える。
- 右側スクロールバー操作など、カーソル位置が変わらない操作ではビューアを変えない。
- `Home` / `End` / キーボード移動 / クリック選択 / ソートやフィルタ後の選択補正など、結果としてカーソルが変わる操作ではビューアも追従する。
- 別ウィンドウ側の前後移動、フォルダ移動、スライドショー、動画連続再生で表示対象が変わった場合、メイン一覧のカーソルも常に追従する。
- 入力中・ドラッグ中・ダイアログ表示中でも論理同期は止めない。ただしフォーカスは奪わず、進行中の操作対象は開始時の `idx` / path に固定する。
- 選択項目が Folder / ZipFile / PdfFile / ConvertibleArchive など、直接表示できない項目の場合、ビューアは現在表示を維持する。`Enter` / ダブルクリックでそのコンテナを開き、表示可能項目へ到達した時点で同期する。
- 見開きで 2 ページ表示している detached session では、メイン一覧側の通常カーソルは現在ページに置き、同時表示中の相方ページがサムネイルグリッド / 詳細一覧の可視範囲内にある場合だけ破線のサブカーソルを描画する。相方が画面外なら描画しない。
- 同期済み判定は `idx` だけでなく `ViewerItemKey` / `items_generation` も見る。同じ raw idx でも item key または generation が変わった場合は、fast-swap の no-op 経路を通さず viewer を再同期する。

### 2.4 動画

- 動画は別ウィンドウでも自動再生する。メイン一覧のカーソル移動で動画に切り替わった場合も自動再生する。
- 高速カーソル移動で動画候補を連続通過する場合は、既存の native video source-swap / pending / most-recent-wins の考え方で最新対象へ集約する。UI スレッドで decoder 完了待ちをしない。
- 画像から動画、動画から画像へ切り替わる場合も、画面上は同じ別ウィンドウセッションの表示対象が変わったものとして扱う。
- detached 動画はメインウィンドウを占有しない。fullscreen / in-window 動画用の main backdrop、main HWND cloak、black chrome、foreground reclaim は detached では発火させず、メイン一覧は通常どおり描画・操作できる状態を保つ。

### 2.5 ウィンドウ管理

- 別ウィンドウは通常の独立 top-level window として扱う。Win32 の owner window にはしない。
- 常に最前面にはしない。他アプリより前に固定しない。
- 初回表示、明示的な open、`F12` で別ウィンドウへ移したタイミングでは必要に応じて前面に出す。
- メイン一覧のカーソル同期だけでは、最小化中の別ウィンドウを勝手に復元したり、他アプリの手前へ出したりしない。
- 最小化・最大化・サイズ変更・移動・`×` は通常の Windows 操作として使えるようにする。
- メインウィンドウの最小化とは連動しない。動画だけ残して再生する用途を許容する。
- アプリ終了時は別ウィンドウも終了する。ただし「閉じるとタスクトレイへ入る」設定で実際にはアプリ終了しない場合は、別ウィンドウと再生を継続する。
- 位置・サイズは保存する。復元時は現在のモニター構成に対して画面外へ出ないよう補正する。

注意: close-to-tray の既存経路は `release_media_session_for_tray()` / `release_gpu_resources()` / UI heartbeat suspend が load-bearing だったため、detached session が開いている場合だけ session と event pump を維持する明示分岐を入れている。
通常の最小化と close-to-tray は別扱いにする。通常最小化では detached window を維持する。close-to-tray では、detached session が開いている場合だけ media session を閉じず、detached 表示のために必要な update / repaint / event pump と `fs_cache` を維持する。

## 3. 状態モデル

### 3.1 用語

- `detached_viewer_enabled`: 別ウィンドウモードが ON かどうか。永続設定。
- `ViewerSession`: 現在開いている画像・動画ビューアのセッション。`×` / `Esc` などで終了する。
- `ViewerPresentation`: 同じセッションをどこに表示しているか。

```rust
enum ViewerPresentation {
    MainWindow,
    Fullscreen,
    DetachedWindow,
}

struct ViewerSession {
    current_idx: usize,
    current_item_key: ViewerItemKey,
    secondary_idx: Option<usize>,
    presentation: ViewerPresentation,
}
```

初期実装では、既存の `fullscreen_idx` を `ViewerSession.current_idx` 相当として段階的に利用する。
一気に全リネームはせず、まずは helper 経由で「ビューア現在項目」を扱う。
`current_item_key` は path / 仮想フォルダ内ページ / media kind などから作る表示対象の同一性キーで、単なる idx ではなく「同じ項目か」を判定するために使う。
`secondary_idx` は永続状態ではなく、見開き解決結果から更新する派生状態として扱う。

### 3.2 推奨 App 状態

- `settings.detached_viewer_enabled: bool`
- `settings.detached_viewer_window_placement: Option<DetachedWindowPlacement>`
- `viewer_presentation: ViewerPresentation`
- `viewer_secondary_idx: Option<usize>`
- `last_viewer_sync_stamp: Option<ViewerSyncStamp>`

`ViewerSyncStamp` は `idx` と `ViewerItemKey` を持つ。`items` rebuild 後は同じ idx が別ファイルを指すため、idx だけで同期済み判定しない。
`items` rebuild をまたぐ可能性がある場合は `last_viewer_sync_stamp = None` にするか、新旧の `ViewerItemKey` を比較して必ず再同期する。

`viewer_session_visible` のような独立 bool は、Phase 1 では原則として持たない。
移行都合で一時的に持つ場合も `viewer_session_visible == fullscreen_idx.is_some()` を不変条件とし、host swap 中も helper 以外から更新しない。
同様に、`viewer_sync_origin` は `last_viewer_sync_stamp` と責務が重複するため初期設計では追加しない。

`DetachedWindowPlacement` は Win32 の `WINDOWPLACEMENT` 相当の restore rect と maximized flag を保存する。
最大化中のウィンドウ矩形をそのまま通常 rect として保存しない。
静止画 egui viewport と native video window の両方で、可能な限り Win32 HWND から placement を取得・復元し、複数 DPI モニターで論理座標の丸め誤差を蓄積させない。

## 4. 内部設計

### 4.1 共通 helper

表示対象更新を以下の helper へ集約する。

- `ViewerOpenReason`
  - `ExplicitOpen`: `Enter`、ダブルクリック、メニューなどユーザーが明示的に開いた。
  - `F12Switch`: 表示中セッションを `F12` で別 presentation へ移した。
  - `SyncFromMainSelection`: メイン一覧の最終 `selected` へ追従した。
  - `ViewerNavigation`: ビューア側の前後移動、連続再生、スライドショーで移動した。
  - `HostSwap`: 同じセッション内で画像 host / 動画 host を差し替えた。
- `open_viewer_for_idx(ctx, idx, reason)`
  - セッションが無ければ作成する。
  - `detached_viewer_enabled` と現在状態から `ViewerPresentation` を決める。
  - 既存の `open_fullscreen(idx)` 相当のロード・キャッシュ・動画起動を呼ぶ。
  - `reason` に応じて window activation 可否を決める。`ExplicitOpen` / `F12Switch` は必要なら前面化可、`SyncFromMainSelection` / `HostSwap` は前面化しない。
- `set_viewer_current_idx(ctx, idx, reason)`
  - `fullscreen_idx` / `ViewerSession.current_idx` を更新する。
  - `selected` / `scroll_to_selected` / last-selected bookkeeping / `ViewerSyncStamp` を同期する。
  - `secondary_idx` を再計算する。
  - root viewport の repaint を要求するが、OS の window activation は行わない。
- `close_viewer_session(reason)`
  - 既存の `close_fullscreen()` のセッション終了処理へ寄せる。
  - 動画再生位置保存、native presenter drop、pending cancel、fs cache cleanup を維持する。
- `sync_open_detached_viewer_to_selected(ctx, reason)`
  - 別ウィンドウセッションが開いている場合だけ、`selected` の変化をビューアへ反映する。
  - セッションが閉じている場合は何もしない。

### 4.2 `fullscreen_idx` の扱い

Phase 1 では `fullscreen_idx` を「ビューアで現在表示している idx」として残す。
ただし新規コードでは直接書き換えず、できるだけ helper 経由に寄せる。

既存の `open_fullscreen` / `close_fullscreen` は大きな責務を持つため、初期実装では名前変更しない。
後続リファクタで `open_viewer` / `close_viewer` へ名称整理する。

ただし、`open_fullscreen` / `close_fullscreen` を detached からそのまま呼ぶだけでは不可。
現状の `open_fullscreen` は main HWND cloak、DWM chrome 変更、foreground reclaim、cursor idle hide reset など、borderless fullscreen takeover 前提の処理を含む。
detached ではこれらが逆効果になるため、detached viewport を配線する前に、表示形態依存の処理を `match viewer_presentation` で分岐させる。

最初に切り出す対象:

- fullscreen / native fullscreen 専用の main HWND cloak。
- DWM chrome 黒化と復元。
- fullscreen foreground reclaim。
- cursor hide / idle reset の適用範囲。
- fullscreen backdrop / holdover の表示条件。
- close 時の native fullscreen 専用 cleanup と detached session close 共通 cleanup の境界。

目標は「ロード・キャッシュ・動画起動・メタデータ読み込み」は presentation-neutral、「ウィンドウ takeover / cloak / focus」は presentation-specific に分けること。

### 4.3 静止画 detached host

- 静止画・ZIP画像・PDFページは既存の `ui_fullscreen.rs::render_fullscreen_viewport` の描画本体を再利用する。
- `ViewerPresentation::DetachedWindow` 用に、装飾付き・taskbar 表示あり・通常ウィンドウサイズの `ViewportBuilder` を追加する。
- 既存の fullscreen viewport builder は borderless / taskbar false のまま維持する。
- `SyncFromMainSelection` / `HostSwap` で新規 detached viewport を作る場合は、`ViewportBuilder::with_active(false)` 相当を使い、作成そのものでフォーカスを奪わない。
- detached viewport の close request は `close_viewer_session("detached-close")` に接続する。
- `Esc` / `Enter` / 右クリックも `close_viewer_session` に接続する。
- detached 中は F11 を処理しない。

### 4.4 動画 detached host

動画は native presenter を使うため、静止画の egui viewport と同じ HWND には載せない可能性が高い。
画面上は 1 つの「別ウィンドウビューア」として扱い、内部では media kind に応じて host を差し替える。

初期実装案:

- `NativeVideoPlacement` を導入する。

```rust
enum NativeVideoPlacement {
    MainWindowChild,
    FullscreenBorderless,
    DetachedWindow { rect: WindowRect },
}
```

- `native_video_presenter_config(..., in_window: bool)` を `NativeVideoPlacement` 受け取りに変更する。
- 旧来の in-window / fullscreen 2 状態だけを扱う bool command / bool event は、detached 実装では drift の原因になる。正本は `NativeVideoPlacement` / `ViewerPresentation` に統一し、cloak / foreground / settings / presenter rebuild の判断を enum へ寄せる。
- `NativeVideoWindowMode` は既に `Windowed` / `Borderless` / `Child` を持つ。detached は既存 `Windowed` を「owner なし + 保存 rect 指定可」に拡張して使うか、新しい `Detached` variant を足すかを Phase 1 で決める。いずれの場合も App 側の正本は `NativeVideoPlacement` に統一する。
- detached 動画では:
  - owner HWND は付けない。
  - main HWND は cloak しない。
  - fullscreen backdrop は出さない。
  - `SyncFromMainSelection` / `HostSwap` で作成・差し替えする場合は `ShowWindow(SW_SHOWNOACTIVATE)` 相当を使い、foreground reclaim / raise を行わない。
  - F11 は無効。
  - `Esc` / `Enter` / 右クリック / `×` は session close。
  - `GeometryChanged` を使って位置・サイズを保存する。
  - HUD overlay は topmost fullscreen 前提を避ける。detached は in-window と同じく fullscreen 用 HUD overlay HWND を使わず、presenter DComp tree 側の overlay 経路を使う。
  - decorated window の `WM_CLOSE` / `Alt+F4` / taskbar close を App 側へ通知する output event を追加し、`close_viewer_session` へ接続する。現状の native window は `WM_CLOSE` で `DestroyWindow` するだけなので、この event path が無いと stale `fullscreen_idx` / stale presenter HWND が残る。
- `toggle_video_window_mode` の Plan B を拡張し、bool command ではなく `SwitchPlacement { request_id, placement, ... }` 相当にする。
  - decoder / audio / clock / source は維持する。
  - 切替完了通知は `PlacementSwitched` に統一する。
  - request id と timeout による stale event 防御は維持する。

画像 detached viewport と動画 native presenter は物理 HWND が異なりうる。
画像 ↔ 動画の切替時は、同じ保存 rect を使って差し替え、ユーザーには同じ別ウィンドウセッションの表示対象が変わったように見せる。
host swap では target 側の作成・初回描画準備ができてから source 側を隠す順序を優先し、真っ黒な 1 フレームやウィンドウ消失感を抑える。
ただしこの swap がメイン一覧からの同期で発生した場合は、target 作成時も no-activate を維持する。

## 5. 同期ルール

### 5.1 メインからビューア

追従判定は `App::update` の終端付近に 1 箇所だけ置く。
`selected` は多数の経路で書き換わり、`rebuild_visible_indices` / `redirect_selected_to_visible` / `ensure_selected_visible_or_first` などでも補正されるため、mid-frame で同期すると補正前 idx を開いてから次フレームで開き直す可能性がある。
必ずそのフレームで確定した最終 `selected` を読んで判定する。

以下の条件で追従をかける。

- `detached_viewer_enabled == true`
- viewer session が開いている
- `selected` の `ViewerSyncStamp` が前回同期した stamp と違う
- `selected` が表示可能項目である
- 変更元がビューア側同期そのものではない

表示可能項目:

- `Image`
- `Video`
- `ZipImage`
- `PdfPage`

`Folder` / `ZipFile` / `PdfFile` / `ConvertibleArchive` / `ZipSeparator` は自動追従対象にしない。
`set_viewer_current_idx` は `selected` と `last_viewer_sync_stamp` を同じタイミングで更新し、viewer → main 同期直後の main → viewer pass が no-op になるようにする。

`items` rebuild 後は bare idx が衝突しやすい。
フォルダ移動、アーカイブ/PDF 仮想フォルダ展開、検索結果/ドライブ一覧への切替、ソート/フィルタで `items` の実体が作り直される経路では、`last_viewer_sync_stamp` を無効化する。
無効化漏れを防ぐため、可能なら `items_generation` を導入して `ViewerSyncStamp { idx, item_key, items_generation }` とし、generation が変わった時点で idx 一致でも再同期する。

### 5.2 ビューアからメイン

以下の経路は必ず `set_viewer_current_idx` を通す。

- 静止画の前後移動、Home/End、固定ジャンプ
- 見開き移動
- 連続読書で中心ページが変わる経路
- Ctrl+↑↓ / Ctrl+PageUp/PageDown のフォルダ移動
- 動画 native presenter の前後移動
- 動画の deferred nav / source swap / continuous playback
- スライドショー

動画側は現状、閉じるタイミングの同期に寄る経路があるため、navigation open / source swap 確定時に明示同期する。

### 5.3 見開きサブカーソル

- `secondary_idx` は `resolve_spread_pair` / `fs_spread_layout` から更新する。
- メイン一覧では `selected` を主カーソル、`secondary_idx` をサブカーソルとして描画する。
- `secondary_idx` がフィルタやフォルダ切替で見えない場合は描画しない。
- 詳細表示モードでは、行の左端アクセントや薄いアウトラインなど、主選択と区別できる表現にする。

## 6. キー操作

- `KeyAction::ToggleDetachedViewerMode` を追加する。
- 既定キーは `F12`。
- `KeyAction::context()` は単一 `KeyContext` しか返せないため、`ToggleDetachedViewerMode` は `KeyContext::Global` として定義する。
- 実際の入力処理では、グリッド、静止画 viewer、native 動画 viewer の各 handler が明示的に `consume_action(ctx, ToggleDetachedViewerMode)` を呼ぶ。`consume_action` 自体は context を見ないため、この形で複数画面に対応できる。
- native presenter では、F12 (`0x7B`) を `native_video_fixed_shortcut_key` の static whitelist に追加する。`GLOBAL_NATIVE_VIDEO_CHORDS` は `FsVideo | Rating` action 由来なので、Global action の F12 転送には使わない。
- F12 は native presenter から App へ転送されるだけでは動かない。App 側の native-video key handler に `0x7B -> ToggleDetachedViewerMode` の明示 branch を追加する。
- 静止画 viewer の F12 は、IME 変換中、モーダルダイアログ中、消しゴム / 隠蔽加工 / テキスト注釈 / 切り取り / 補正レイヤーなどの overlay edit mode 中は発火させない。Phase 2 以降で detached host へ編集状態を移す設計が固まるまでは、編集中の host 切替は未定義にしない。
- detached 動画で `Enter` を session close にする場合、Enter (`0x0D`) も native presenter から App へ届く必要がある。static whitelist へ追加するか、detached placement の native wndproc 側で close event として扱う。
- 既存の F11 は以下の扱いにする。
  - `MainWindow` / `Fullscreen`: 既存の静止画 in-window ↔ fullscreen 切替を `ViewerPresentation::{MainWindow, Fullscreen}` の遷移として扱う。
  - `DetachedWindow`: 無効。動画 native presenter 側でも無効。
- detached session を `×` / `Esc` / `Enter` / 右クリックで閉じても `detached_viewer_enabled` は維持する。次に開く操作をした場合は再び `DetachedWindow` で開く。
- non-detached の `Fullscreen` での `Esc` は既存どおり一覧へ戻る挙動を維持する。detached だけは `Esc` を session close として扱う。
- keymap 追加時は `ALL_ACTIONS`、`ini_name`、`default_chords`、`trigger`、ini round-trip / default generation のテストを更新する。

## 7. ウィンドウとフォーカス

- detached window は owner を持たない top-level window とする。
- open / F12 switch のような明示操作では show/raise してよい。
- メイン一覧の選択同期だけでは focus / foreground を奪わない。既存 window を raise しないだけでなく、同期に伴う画像 ↔ 動画 host の新規作成でも no-activate で表示する。
- detached window が最小化中なら、選択同期だけで復元しない。
- detached window 側を操作したときは通常の Windows フォーカス規則に従う。
- メインウィンドウ最小化とは連動しない。
- アプリ終了時は全 viewer session を閉じる。detached window は owner-less なので、native video window も含めて exit path (`on_exit` / Drop 相当) で明示的に destroy し、presenter thread を join する。
- close-to-tray 設定でメインを閉じてもプロセスが継続する場合は、detached window と動画再生も継続する。
- close-to-tray では、detached session が開いている場合に限り `release_media_session_for_tray()` が `close_fullscreen()` を呼ばないようにする。
- close-to-tray 中も detached 表示に必要な UI heartbeat / event pump を止めない。静止画 detached は egui viewport を `App::update` から描くため、heartbeat suspend のままでは表示が凍る。native 動画も presenter event / geometry save / source swap completion を App 側が pump できなくなる。
- `release_gpu_resources()` は detached 表示中の active `fs_cache` / detached image texture を破棄しない。必要ならグリッドサムネイルだけを解放する。

## 8. 設定とドキュメント更新

実装時に更新する。

- `settings.rs`
  - `detached_viewer_enabled`
  - `detached_viewer_window_placement`
- `docs/spec.md`
  - 「アプリケーションウィンドウは 1 つ」の記述を更新する。
  - F12 / detached mode / close-to-tray 時の再生継続を追加する。
- `docs/keymap-spec.md`
  - F12 と detached 中 F11 無効を追加する。
- `docs/display-pipeline.md`
  - `ViewerPresentation` と detached viewport を追加する。
- `docs/video-architecture.md`
  - `NativeVideoPlacement::DetachedWindow` と HUD overlay 方針を追加する。
- `docs/async-architecture.md`
  - `SwitchPlacement`、placement pending、most-recent-wins、native `WM_CLOSE` output event を追加する。
- `docs/ui-responsiveness.md`
  - end-of-update の選択同期、カーソル移動による動画自動再生、source-swap / pending 経由で UI スレッドを待たせない方針を追加する。
- `CLAUDE.md`
  - 「アプリケーションウィンドウは 1 つ」系の運用メモがあれば、detached viewer 仕様に合わせて更新する。
- `htdocs/mimageviewer/manual/`
  - ショートカット、フルスクリーン/別ウィンドウ説明、設定説明を更新する。

## 9. 実装フェーズ

### Phase 0: 設計レビュー

- ClaudeCode 初回レビュー済み。P0/P1 指摘は本計画へ反映済み。
- ClaudeCode 再レビュー済み。P1-A/P1-B と P2 の clarifications も本計画へ反映済み。

### Phase 1: 状態・キー・設定

- `ViewerPresentation` / `NativeVideoPlacement` / helper / settings を追加する。
- `ViewerItemKey` / `ViewerSyncStamp` を追加し、main → viewer 同期ガードを bare idx ではなく item identity で判定できるようにする。
- 既存の `video_in_window_mode` / `native_video_in_window_active` を 3 状態 enum へ寄せる migration 方針を決め、bool と enum が drift しない構造にする。
- `KeyAction::ToggleDetachedViewerMode` を `KeyContext::Global` として追加し、F12 既定割り当てを追加する。
- keymap の `ALL_ACTIONS`、ini 名、default chord、trigger、ini round-trip / default generation テストを更新する。
- native presenter の static VK whitelist に F12 を追加し、App 側 native-video key handler に F12 dispatch branch を追加する。
- F12 で設定だけ切り替わる状態を先に作る。
- F11 の detached 無効化ガードを追加する。

進捗メモ:

- `settings.detached_viewer_enabled`、`KeyAction::ToggleDetachedViewerMode`、F12 既定割り当て、grid / 静止画 viewer / native 動画 viewer での F12 dispatch は実装済み。
- `ViewerPresentation::{MainWindow, Fullscreen, DetachedWindow}` と `App.viewer_presentation` は導入済み。`requested_viewer_presentation_for_open` は `detached_viewer_enabled` を見て `DetachedWindow` 要求を返す。
- 静止画 / ZIP画像 / PDFページ / 動画はいずれも `effective_viewer_presentation_for_open` で `DetachedWindow` を実表示先として採用する。
- native 動画は `NativeVideoPlacement::{MainWindowChild, FullscreenBorderless, DetachedWindow}` を正本にし、旧 bool command / bool event は削除済み。

### Phase 2: presentation-neutral 化 + 静止画 detached + tray 対応

- `open_fullscreen` / `close_fullscreen` の presentation-specific block を先に切り出す。
- main HWND cloak、DWM chrome、foreground reclaim、cursor hide、fullscreen backdrop を `ViewerPresentation` で分岐させる。
- `ViewerOpenReason` ごとに activation 可否を分岐し、同期由来の detached window 作成・host swap は no-activate にする。
- close-to-tray 中に detached session を維持するため、`release_media_session_for_tray`、UI heartbeat、`release_gpu_resources` の扱いを変更する。
- 静止画・ZIP画像・PDFページを detached viewport に描画する。
- close / Esc / Enter / right-click で session close。
- メイン選択との双方向同期を `App::update` 終端の単一 choke point で実装する。
- 位置・サイズ保存と画面外補正を実装する。

進捗メモ:

- `open_fullscreen` の main HWND cloak / DWM chrome / foreground reclaim clear は `prepare_viewer_presentation_open` へ切り出し済み。
- `close_fullscreen` の native 動画 fullscreen close cleanup / foreground reclaim 予約は `prepare_viewer_presentation_close` へ切り出し済み。
- 静止画 / ZIP画像 / PDFページの detached viewport は実装済み。既存の `render_fullscreen_viewport` 描画本体を、装飾付き・taskbar 表示あり・通常サイズの viewport に出す。
- detached 静止画 session 中はメインウィンドウをブロックしない。メイン root に届いたキーは fullscreen root handler が横取りせず、グリッド側へ流す。
- detached 静止画の `×` / `Esc` / `Enter` / 右クリックは `close_fullscreen()` に寄せ、`detached_viewer_enabled` は維持する。detached 中の F11 は no-op にし、ホバーバーの window/fullscreen トグルは非表示にする。
- detached session が開いている間、`App::update` 終端で最終 `selected` を見て、静止画 / ZIP画像 / PDFページ / 動画なら viewer を追従させる。同期済み判定は `ViewerSyncStamp { idx, item_key, items_generation }` で行い、bare idx のみでは判定しない。
- detached session が閉じている場合は、メイン一覧のカーソル移動だけでは再表示しない。
- 表示中セッションの F12 host migration は実装済み。静止画は egui viewport の表示先を切り替え、動画は `SwitchPlacement` で decoder / audio / clock を保持したまま native HWND を作り直す。
- 同期由来の detached open / host swap は no-activate で表示し、通常の open / F12 操作では必要に応じて前面化する。
- detached window placement は `detached_viewer_window_placement` に保存する。意味は「outer position + inner/client size + maximized flag」。最大化中は restore placement を上書きせず、`maximized` だけを更新する。
- close-to-tray 中に detached session が開いている場合は、`release_media_session_for_tray` で `close_fullscreen()` を呼ばず、UI heartbeat と active viewer cache を維持する。通常 fullscreen / 通常動画は従来通り tray hide 時に閉じる。

### Phase 3: 動画 detached

- detached native window を通常 top-level window として作る。
- `SwitchPlacement` で再生維持したまま MainWindow / Fullscreen / Detached を切り替える。
- decorated detached native window の `WM_CLOSE` / `Alt+F4` / taskbar close を App へ通知する event path を追加する。
- detached 動画で `Enter` close を使うため、Enter の native key 転送または native wndproc close 処理を追加する。
- detached 動画で close / Esc / Enter / right-click を session close に接続する。
- 動画前後移動・連続再生・deferred nav のメイン同期を確実にする。
- アプリ終了 path で owner-less detached native video window を明示 destroy し、presenter thread を join する。

進捗メモ:

- `NativeVideoWindowMode::WindowedAt`、`NativeVideoPlacement::DetachedWindow`、`SwitchPlacement` / `PlacementSwitched` / `PlacementSwitchFailed` を追加済み。
- detached 動画 window は owner なしの通常 top-level window として作成し、F11 は detached 中 no-op、F12 は再生を維持した host migration として扱う。
- detached 動画 window の `WM_CLOSE` は `NativeVideoWindowEvent::CloseRequested` として App へ転送し、`close_fullscreen()` で session 終了・動画停止に寄せる。detached 動画の Esc / Enter も同じ close 経路に入る。
- detached 動画では fullscreen 専用 HUD overlay HWND / fullscreen backdrop / VST owner 同期を使わず、通常 presenter HWND 側の overlay path を使う。
- 残りは Windows 実機での複数ディスプレイ / DPI 差 / close-to-tray / 動画連続再生の確認。

### Phase 4: 画像・動画横断と見開き

- 画像 ↔ 動画の host 差し替えを同一 geometry で行う。
- `secondary_idx` とサブカーソル描画を追加する。
- フォルダ移動・検索結果・ZIP/PDF 仮想フォルダを通した同期を確認する。

### Phase 5: ドキュメント・テスト・実機検証

- `spec.md`、`display-pipeline.md`、`video-architecture.md`、`keymap-spec.md`、manual を更新する。
- App-level 状態機械テストを追加する。
- UI snapshot はサブカーソルなど見た目が変わる箇所に追加する。
- Windows 実機で複数ディスプレイ、DPI 差、最小化、close-to-tray、動画連続再生を確認する。

## 10. テスト観点

- F12 ON/OFF が keymap default と user ini で動く。
- F12 追加後も `ALL_ACTIONS` と enum が同期し、default ini / user ini round-trip が通る。
- native 動画ウィンドウに focus がある状態で F12 が App へ転送され、detached mode toggle が実行される。
- detached mode ON、session closed の状態でカーソル移動しても window が開かない。
- detached mode ON、session open の状態でカーソル移動すると viewer が追従する。
- 選択同期は `App::update` 終端の最終 `selected` だけを見て動き、非表示項目やコンテナ項目では viewer を開き直さない。
- detached session open 中に folder / archive / search result などで `items` rebuild が起き、旧 selected と新 selected の idx が同じでも、`ViewerItemKey` / generation 差で viewer が新しい項目へ再同期する。
- メイン一覧の同期で画像 ↔ 動画 host swap が発生しても、detached window がメインウィンドウや他アプリから foreground を奪わない。
- detached viewer の `×` / `Esc` / `Enter` / 右クリックが session close と動画停止になる。
- decorated detached 動画ウィンドウの `×` / `Alt+F4` / taskbar close が App 側 `close_viewer_session` へ届き、stale `fullscreen_idx` / stale presenter HWND が残らない。
- `×` 後も detached mode は維持され、次の open で window が再表示される。
- 動画再生中に F12 を押しても再生位置・再生状態が維持される。
- detached 中の F11 が何もしない。
- スライドショー / 動画連続再生で viewer が進むとメイン一覧カーソルも進む。
- メインを最小化しても detached 動画再生が続く。
- plain minimize と close-to-tray を区別して検証する。
- close-to-tray 設定でメインを閉じても detached 静止画 viewport が凍らず、detached 動画再生と presenter event pump が続く。
- close-to-tray 中に detached active `fs_cache` が破棄されない。
- アプリ終了では detached window も終了する。
- アプリ終了 path で owner-less native video window が明示 destroy され、presenter thread が残らない。
- 保存 placement が画面外なら補正される。
- 最大化中に閉じた detached window を再表示した時、restore rect と maximized flag が分離して復元され、最大化サイズの通常ウィンドウにならない。
- 画像 ↔ 動画切替で window 位置・サイズが大きく飛ばない。
- フォルダ / ZIP / PDF / 検索結果 / 詳細表示モードで同期が破綻しない。

## 11. ClaudeCode レビュー反映メモ

初回 ClaudeCode レビューで出た load-bearing 指摘は本計画へ反映済み。

- P0: close-to-tray は `release_media_session_for_tray()` / heartbeat suspend / `release_gpu_resources()` による teardown が load-bearing。detached session 中だけ media session / heartbeat / active viewer cache を維持する分岐を入れた。
- P1: `video_in_window_mode` / `native_video_in_window_active` と新 enum を並走させると drift する。実装では `NativeVideoPlacement` を正本にし、旧 bool command / bool event は削除した。
- P1: `open_fullscreen` / `close_fullscreen` は fullscreen takeover 処理を含むため、detached viewport 配線前に presentation-specific block を切り出す。
- P1: F12 は static VK whitelist 追加だけでなく、App 側 native-video key handler の dispatch branch が必要。
- P1: `KeyAction::context()` は単一 context なので、F12 は `Global` action として定義し、各 handler で明示 consume する。
- P1: decorated detached 動画 window の `WM_CLOSE` を App へ返す event path が必要。
- P1: main → viewer 同期は `App::update` 終端の単一 choke point に置く。
- P2: detached 動画 HUD は fullscreen 用 topmost HUD HWND を避け、in-window と同じ DComp overlay path を使う。
- P2: detached 動画で Enter close を採用するなら Enter の native key 転送も必要。
- P2: image viewport と native video HWND の host swap には 1 フレーム程度の gap がありうる。必要なら target 表示準備後に source を隠す順序を検討する。
- P2: rect 復元は既存 monitor helper を拡張し、画面外 clamp を実装する。

再レビューで出た実装前固定項目も反映済み。

- P1-A: `last_viewer_synced_selected: Option<usize>` のような bare idx guard では、`items` rebuild 後に同じ idx が別項目を指すケースを見逃す。`ViewerItemKey` / `ViewerSyncStamp` を使うか、rebuild 経路で stamp を無効化する。
- P1-B: 同期由来の画像 ↔ 動画 host swap では、新規 top-level window 作成そのものが focus を奪う可能性がある。`ViewerOpenReason` に activation 可否を持たせ、egui viewport / native video window とも no-activate 作成を使う。
- P2: `viewer_sync_origin` は `ViewerSyncStamp` と責務が重複するため、初期設計では追加しない。
- P2: `viewer_session_visible` は `fullscreen_idx.is_some()` と一致する不変条件にし、独立状態として drift させない。
- P2: detached geometry は最大化 rect と restore rect を分け、Win32 `WINDOWPLACEMENT` 相当で保存・復元する。
- P2: owner-less detached window はアプリ終了時に明示 destroy / presenter thread join が必要。
- P2: 既存の静止画 F11 は `ViewerPresentation::{MainWindow, Fullscreen}` の遷移として扱い、detached では F11 を無効にする。

更新後に再レビューする場合の依頼文:

```
docs/detached-viewer-implementation-plan.md の更新後レビューをお願いします。

初回レビューで指摘された close-to-tray teardown、fullscreen_idx/open_fullscreen/close_fullscreen の presentation-specific block、NativeVideoPlacement への enum 集約、F12 native dispatch、decorated detached video window の WM_CLOSE event、end-of-update selection sync を計画に反映しました。

重点確認:

- Phase 1〜3 の順序で、enum migration / presentation-neutral extraction / tray survival / still detached / video detached の依存関係に無理がないか。
- `NativeVideoPlacement` を正本にする方針で、既存の `video_in_window_mode` / `native_video_in_window_active` 由来の drift を避けられるか。
- close-to-tray 中に detached image viewport と detached native video playback を維持するための heartbeat / fs_cache / event pump 方針に不足がないか。
- end-of-update の main→viewer sync と viewer→main sync の feedback loop 防止が十分か。
- F12 Global action、static VK whitelist、App-side native dispatch、Enter close の native 転送に漏れがないか。
- decorated detached native video window の close event と stale presenter / stale fullscreen_idx 防止の設計に穴がないか。
- 追加すべきテスト、docs、実機検証項目が残っていないか。
```
