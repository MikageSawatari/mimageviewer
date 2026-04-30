# Codex 回答: VST3 不具合 4 件の調査結果

前提: この調査はコードリーディングベースです。手元で SSL Meter Pro / Insight2 の実機再現はしていません。

## 不具合 1 (P1): z-order
- 場所: `src/video/dsp/mod.rs:345` `show_slot_gui`, `src/video/dsp/mod.rs:488` `set_all_guis_visible`, `src/video/dsp/mod.rs:506` `set_all_guis_topmost`, `src/video/dsp/gui.rs:528` `set_window_topmost`
- 原因: 既存 HWND の再表示パスは `src/video/dsp/mod.rs:357-365` で `ShowWindow(SW_SHOWNA)` のみなので、ここ単体では z-order を積極的に壊していない。一方で `set_all_guis_topmost` は `src/video/dsp/mod.rs:507-518` で slot 順に `SetWindowPos(HWND_TOPMOST/HWND_NOTOPMOST)` を呼ぶだけで、ユーザーが手で並べた相対 z-order を保存・復元していない。複数 HWND に対する `SetWindowPos(HWND_TOPMOST)` は呼んだ順に上へ積まれるため、fullscreen 入退出や一括表示で slot 順へ寄りやすい。
- 修正案: mIV 管理下の GUI HWND 群について z-order snapshot を持つ。`GetWindow(GW_HWNDNEXT)` で desktop の top-to-bottom 順を走査し、slot の HWND に該当するものだけを保存する。hide 前、`set_all_guis_topmost(true/false)` 前、一括 show 前に snapshot し、show/topmost 切替後に bottom-to-top 順で `SetWindowPos` / `DeferWindowPos` して復元する。
- 実装メモ: `set_all_guis_topmost(true)` は復元時に bottom-to-top で `HWND_TOPMOST`、`set_all_guis_topmost(false)` はまず `HWND_NOTOPMOST` を付けた後、bottom-to-top で `HWND_TOP` + `SWP_NOACTIVATE` に並べ直すのが安全。`SW_SHOWNA` のみの再表示パスは維持してよいが、複数 GUI 一括操作の直後に restore を挟む。

## 不具合 2 (P1): 右クリックメニュー
- 場所: `src/video/dsp/gui.rs:169-180` `WM_PARENTNOTIFY`, `src/video/dsp/gui.rs:449-465` host window 作成, `crates/vst3-host/src/plugin_loader.cpp:417-436` `setFrame` + `attached`
- 原因: 現在の対策は mIV 側 host window の `WM_PARENTNOTIFY` に依存して `SetForegroundWindow(host_hwnd)` を呼ぶだけ。プラグインの child / descendant window が `WS_EX_NOPARENTNOTIFY` を持つ場合、右クリックが親へ届かない。さらに入力を実際に受けているのは bridge プロセス側の plugin child なので、mIV プロセス側からの `SetForegroundWindow` は foreground lock の都合で失敗しうる。結果として `TrackPopupMenu` の owner top-level が foreground でない状態になり、SSL Meter Pro などの popup が即 dismiss される構造が残る。
- 修正案: bridge 側で `attached()` 後に `EnumChildWindows(host_hwnd)` し、同一プロセスに属する plugin child / descendant を subclass する。subclass wndproc で `WM_MOUSEACTIVATE`, `WM_RBUTTONDOWN`, `WM_CONTEXTMENU` を捕まえ、入力を受けた bridge プロセス側から `SetForegroundWindow(host_hwnd)` を呼ぶ。mIV 側の `WM_PARENTNOTIFY` は fallback として残す。
- 実装メモ: cross-process window subclass は避ける。`GetWindowThreadProcessId` で current process の child だけ対象にする。plugin が後から child を作る場合があるため、`WM_PARENTNOTIFY` / WinEvent hook / short timer で再 enum できる形にすると堅い。host 側には `WM_MOUSEACTIVATE` で `MA_ACTIVATE` を返す fallback も追加候補。

## 不具合 3 (P2): fullscreen で VST ボタン 2 回目以降 show fail
- 場所: `src/ui_fullscreen.rs:1169-1177` VST ボタン処理, `src/app.rs:13143-13155` fullscreen 入退出時 TOPMOST 切替, `src/video/dsp/mod.rs:357-365` 既存 GUI 再表示パス, `src/video/dsp/gui.rs:425` TOPMOST なしで window 作成
- 原因: `set_all_guis_topmost(true)` は `src/app.rs:13144-13145` の fullscreen 状態遷移時にしか呼ばれない。fullscreen に入った後で VST ボタン #1 により初めて GUI を作る場合、その HWND は `src/video/dsp/gui.rs:425` の通り TOPMOST なしで作られ、作成直後の z-order 偶然で見えることがある。#2 で `SW_HIDE`、#3 で `SW_SHOWNA` すると、再表示パスは `src/video/dsp/mod.rs:357-365` で TOPMOST を再付与しないため、fullscreen viewport の背面に残って「表示されない」状態になる。
- 修正案: fullscreen 中に GUI を開く経路でも TOPMOST desired state を必ず適用する。最小修正は `src/ui_fullscreen.rs:1172` の `set_all_guis_visible(opening)` の直後、`opening == true` のときに `self.dsp_bridge.set_all_guis_topmost(true)` を呼ぶこと。ただし不具合 1 の z-order 復元と組み合わせないと slot 順へ並び直る。
- より堅い案: `DspBridge` 側に `gui_topmost_desired: AtomicBool` のような状態を持たせ、`set_all_guis_topmost` で保存する。`show_slot_gui` の新規作成後と既存 HWND 再表示後に desired state を適用する。UI 層の呼び忘れを避けられるため、こちらを推奨。

## 不具合 4 (P2): Insight2 リサイズ時の振動・追従遅れ
- 場所: `src/video/dsp/gui.rs:194-210` `WM_SIZE`, `src/video/dsp/mod.rs:538-584` resize signal pump, `crates/vst3-host/src/plugin_loader.cpp:460-472` `notify_host_resize`, `crates/vst3-host/src/host_app.cpp:96-142` `PlugFrame::resizeView`
- 原因: Rust host は `WM_SIZE` を受けるたびに最新値を bridge へ送り、bridge は `notify_host_resize` で `mark_user_resize()` してから `view->onSize` を呼んでいる。`resizeView` 側は直近 250ms のみ `SetWindowPos` を抑止するが、Insight2 は同期・非同期に `resizeView` を返すため、250ms を外れた遅延 callback が host window のサイズを plugin 推奨値へ戻し、ユーザー drag と競合する。これが「内容が遅れる / 揺れる」主因。
- 修正案: タイムスタンプ抑止を主制御にせず、`WM_ENTERSIZEMOVE` / `WM_EXITSIZEMOVE` ベースの明示的な user resize session にする。host wndproc が enter で bridge に `begin_host_resize`、exit で最後の client size とともに `end_host_resize` を送る。bridge / `PlugFrame` は session 中の `resizeView` では host HWND への `SetWindowPos` を常に禁止し、`view->onSize` の受領だけ返す。exit 後に final size を一度 `view->onSize` し、その後だけ plugin 主導 resize を再許可する。
- 実装メモ: drag 中も表示追従を維持したい場合は `WM_SIZE` 最新値を frame 単位で `notify_host_resize` してよいが、`resizeView -> SetWindowPos` は session 中ずっと禁止する。最終 `onSize` 後に `InvalidateRect` / `RedrawWindow(RDW_INVALIDATE | RDW_ALLCHILDREN)` を追加すると、Insight2 の stale frame を押し出せる可能性がある。現行の 250ms ガードは fallback として残せるが、主制御からは外す。

## 全体構造への提案
- GUI HWND の状態は `visible` だけでなく、`topmost_desired` と `z_order_snapshot` を DspBridge 側に寄せる。fullscreen UI / manager UI の各呼び出し元に TOPMOST 再適用を分散させない。
- z-order 復元、TOPMOST 切替、show/hide は一つの helper に集約する。`set_all_guis_visible(true)` と `set_all_guis_topmost(true/false)` が別々に HWND を触る現在の形だと、今後も順序依存のバグが出やすい。
- 右クリック foreground 対策は mIV 親 window だけでなく、bridge 側 child window の入力経路で処理する。popup 系の不具合は plugin ごとの差が大きいため、親通知だけを唯一の経路にしない方がよい。
- resize は「ユーザーが drag 中か」を Win32 の move/size modal loop で明示的に管理する。VST3 の `resizeView` は plugin からの要求であり、host ユーザー resize 中に host size を戻す権限を持たせると競合する。
