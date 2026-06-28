# Detached Window Current Behavior Investigation

作成日: 2026-06-28

目的: PDF / ZIP / 画像を複数の detached window で開く実装について、現在の state machine と
実機ログ上の挙動を整理する。これは修正方針の確定前に ClaudeCode へレビューしてもらうための
調査メモであり、まだ修正案の確定版ではない。

## 1. 現在のモデル

### 1.1 active detached viewer

- active viewer は `active_detached_viewer_context: Option<ActiveDetachedViewerContext>` に
  `ViewerContextBundle` として保持される。
- `update_active_detached_viewer_context()` は active bundle を一時的に `App` へ mount し、
  enumerate / prefetch / AI / fullscreen 描画 / close 処理を active context として実行する。
- detached 静止画 viewer の viewport id は `fullscreen_viewport_id()` で決まる。
  `viewer_presentation == DetachedWindow` かつ `detached_viewer_window_id` がある場合は、
  `detached_image_window_viewport_id(detached_viewer_window_id)` を返す。
- active viewer の Win32 HWND は `detached_viewer_host_hwnd` に保持される。この field は
  `ViewerContextBundle` に含まれており、active context mount / unmount の swap 対象になっている。

### 1.2 passive / paused detached window

- passive window は `detached_image_windows: Vec<DetachedImageWindowSnapshot>` に入る。
- snapshot は次を持つ。
  - stable `id`
  - 表示用 texture / frozen continuous pages
  - placement
  - reopen descriptor
  - optional `paused_bundle`
  - focus / activation 状態
  - `initial_placement_applied`
- passive 描画は `render_detached_image_windows()` が main context 側で行う。
- passive viewer も active 時と同じ `detached_image_window_viewport_id(id)` を使うため、
  設計上は active / passive を切り替えても OS window を作り直さない想定。

### 1.3 open / pause / reactivate / close

新しい本を main grid から開く:

1. `open_grid_container_in_detached_book_context()` が呼ばれる。
2. 既存 active があれば `park_and_close_current_active_detached_viewer()` を呼ぶ。
3. 通常は `pause_current_active_detached_viewer_context()` が active bundle を snapshot の
   `paused_bundle` として退避する。
4. `start_active_detached_book_context_from_descriptor()` が新しい active context を作り、
   新しい `detached_viewer_window_id` を割り当てる。

passive window を再アクティブ化する:

1. `activate_detached_image_window_snapshot()` が snapshot を `detached_image_windows` から remove する。
2. `paused_bundle` がある場合、現在 active を pause する。
3. `paused_bundle.detached_viewer_window_id = Some(snapshot.id)` として同じ stable viewport id へ戻す。
4. `active_detached_viewer_context = Some(paused_bundle)` に戻す。
5. descriptor fallback の場合だけ passive viewport へ `ViewportCommand::Close` を送り、再列挙する。

active detached window を閉じる:

1. active viewport で close request が来る。
2. active context 内で `close_fullscreen()` が走り、`fullscreen_idx` が `None` になる。
3. `finalize_closed_active_detached_viewport()` が active viewport へ `ViewportCommand::Close` を送り、
   `fs_viewport_shown=false`, `fs_viewport_presentation=None`, `detached_viewer_host_hwnd=0` にする。
4. active context が drop される。
5. 現在の実装では、active context が残っていなければ main/root viewport へ Focus を 1 回送る。

## 2. 直近ログの重要部分

ログ: `%APPDATA%\mimageviewer\logs\mimageviewer.log`
最終更新: 2026-06-28 16:54:18

### 2.1 open 時

```text
5.307 show viewport: generation=1 host=hwnd=0
5.346 captured host hwnd=0xe116b8 rect=(2273,587)-(4210,1834) ppp=1.50
6.620 show viewport: generation=2 host=hwnd=0
6.661 captured host hwnd=0x35b1370 rect=(2333,647)-(4270,1894) ppp=1.50
10.137 show viewport: generation=3 host=hwnd=0
10.179 captured host hwnd=0x201762 rect=(282,383)-(2219,1630) ppp=1.50
```

3 つの active window が順に作られ、各 active context が HWND を捕捉している。

### 2.2 active close 後に passive window のサイズが変わる

```text
13.237 viewport close_requested: host=hwnd=0x201762 rect=(856,506 1937x1247)
13.243 active_close_finalize begin ... passive_windows=2 host=hwnd=0x201762 ...
13.243 main_focus_after_close reason=active_close_finalize passive_windows=2
13.628 passive_event id=4 focused=true ... focus_suppressed=true ...
13.628 passive_focus_activation_ignored id=4 suppressed=true ...
13.628 passive_placement_update id=4 initial_apply=false
       from=DetachedViewerWindowPlacement { x: 148.0, y: 215.33333, w: 1276.6666, h: 794.00006, maximized: false }
       to=DetachedViewerWindowPlacement { x: 126.666664, y: 126.666664, w: 533.3333, h: 400.0, maximized: false }
13.843 passive_placement_update id=4 initial_apply=false
       from=(126.666664,126.666664 533.3333x400.0)
       to=(152.0,152.0 533.3334x400.0)
14.162 passive_placement_update id=4 initial_apply=false
       from=(152.0,152.0 533.3334x400.0)
       to=(177.33333,177.33333 533.3333x400.0)
14.408 passive_placement_update id=4 initial_apply=false
       from=(177.33333,177.33333 533.3333x400.0)
       to=(202.66667,202.66667 533.3333x400.0)
```

重要点:

- `initial_apply=false` なので、アプリは「この passive viewport には初回 placement 適用済み」と見ている。
- しかし実際の `outer_rect` / `inner_rect` は 1276x794 logical から 533x400 logical へ変化している。
- 533x400 logical は ppp=1.50 の環境で 800x600 pixel 相当で、egui / OS のデフォルト window size に見える。
- その後、位置が約 25 logical px ずつ cascade している。これは「既存 window を維持した」のではなく、
  OS / egui が新規 window として既定配置している挙動に近い。

### 2.3 passive reactivation 後に active viewport が host lost で再生成される

```text
22.573 passive_event id=4 focused=true ... pointer_activation=true ... user_activation=true
22.573 passive_activate_queued id=4 via=pointer passive_windows=1 active_context=false
22.573 passive_activate_committed id=4 passive_windows=0 active_context=true
22.625 recreate viewport: reason=host_lost_before_render fullscreen_idx=Some(0)
       viewer_presentation=DetachedWindow fs_shown=true fs_presentation=Some(DetachedWindow)
       generation=2 host=hwnd=0x35b1370 alive=false visible=false iconic=false rect=<none>
22.625 clear host hwnd=0x35b1370
22.644 show viewport: fs_idx=0 activate=Some(true) generation=3 host=hwnd=0
22.703 captured host hwnd=0xe216b8 rect=(304,304)-(1126,960) ppp=1.50
```

重要点:

- id=4 の passive reactivation は pointer 操作で正常に始まっている。
- しかし active context に戻した直後、`detached_viewer_host_hwnd=0x35b1370` が dead と判定され、
  `host_lost_before_render` で active viewport を recreate している。
- これは「stable viewport id で同じ OS window を継続する」設計と矛盾する。
- captured rect は `(304,304)-(1126,960)` で、当初の大きい window rect とは異なる。
  前段で snapshot placement が 533x400 logical に書き換わっていた影響を受けている可能性がある。

## 3. 現時点の問題仮説

### 3.1 根本仮説: stable viewport id と HWND / viewport lifetime の管理単位がずれている

設計上は detached window ごとに stable viewport id を持つ。しかし実装上の HWND tracking は
`detached_viewer_host_hwnd` として active context bundle に含まれている。

そのため:

- active から passive へ pause した後、passive window は main context の
  `render_detached_image_windows()` で描画される。
- しかし passive window の実 HWND は snapshot 側に追跡されない。
- paused bundle 内の `detached_viewer_host_hwnd` は、active だった時点の HWND のまま残る。
- その HWND が OS / egui 側で破棄、再生成、または別 HWND 化しても、paused bundle は更新されない。
- reactivation 時に bundle を active context へ戻すと、active 側は stale HWND を見て
  `host_lost_before_render` を起こし、viewport を recreate する。

ログの `host=hwnd=0x35b1370 alive=false` はこの仮説と一致する。

### 3.2 pause snapshot の placement 取得元が live OS rect ではない

`build_active_detached_image_window_snapshot()` は snapshot の placement を
`detached_viewer_window_placement()` から取る。この関数は
`settings.detached_viewer_window_placement` を正とし、無ければ default placement を返す。
つまり、active window を paused snapshot 化するときに、その瞬間の OS window live outer rect を
必ず読んでいるわけではない。

直近ログの 13.628s の resize は、この影響で説明できる可能性が高い。

- active window の実 rect は 1276x794 logical 付近だった。
- snapshot に入った placement は settings/default 由来で 533x400 logical 付近だった。
- passive render で snapshot placement が viewport へ適用され、window が 533x400 へ縮んだ。
- その default geometry が `placement_update` で snapshot へ保存され、以後の正として扱われた。

したがって、default geometry 化の一次原因は「viewport 再生成」だけではなく、
pause 時の placement capture が live OS rect ではないことにある。

### 3.3 `initial_placement_applied` は OS viewport の実生存と結びついていない

`initial_placement_applied` は snapshot 内の bool であり、次を表すに過ぎない。

- 以前 `show_viewport_immediate` の builder に placement を渡したことがある。

しかし必要なのは次の情報である。

- 現在の stable viewport id に対応する OS viewport / HWND が本当に生きているか。
- 生きていない場合、次回 render は新規作成なので placement を再適用すべきか。
- 生きている場合、OS の live geometry を authority として app placement へ書き戻すべきか。

現行実装では viewport が再生成されても `initial_placement_applied=true` のまま残り得る。
その場合、builder は position / size を渡さず、egui / OS のデフォルト 800x600 相当で window が作られる。
その default geometry を `placement_update` で snapshot へ保存してしまうため、以後の active reactivation も
小さい window サイズを正として扱う。

### 3.4 close 後 focus main は症状を抑える目的だが、根本状態不整合の解決ではない

`main_focus_after_close` は OS focus が残存 passive window 間を渡り歩く見た目のちらつきを抑えるために
追加した。しかし、今回のログでは focus 誘導後に passive placement が default size へ変化している。

これは focus 誘導自体が主原因とは限らないが、少なくとも:

- active close
- active context drop
- main focus
- passive render
- passive viewport が default geometry を報告

という順で観測されている。focus 制御は HWND / viewport lifetime の不整合を隠せず、むしろ再生成タイミングを
見えやすくしている可能性がある。

## 4. 現在の不変条件と破れている疑い

| 不変条件 | 現状 |
| --- | --- |
| detached window id 1 つにつき、active / passive をまたいで同じ OS window を継続する | `host_lost_before_render` により破れている疑い |
| app 側の placement は、初回作成時の seed または live OS geometry のどちらか一方を authority とする | pause 時に settings/default 由来 placement を snapshot へ入れ、さらに default geometry を live geometry と誤認して保存している疑い |
| `initial_placement_applied=true` は「現在の OS viewport に placement 済み」を意味する | 実際には「過去に一度 builder へ渡した」だけで、現在 viewport との対応を保証しない |
| paused bundle の `detached_viewer_host_hwnd` は reactivation 時に有効 | ログ上 dead HWND が戻ってきている |
| passive render は window を再生成しない | ログ上 default size / cascade placement が出ており、再生成相当の挙動が見える |

## 5. 修正前にレビューしたい論点

### 5.1 HWND tracking を bundle ではなく per detached window runtime へ移すべきか

現在 `detached_viewer_host_hwnd` は `ViewerContextBundle` に含まれる。
しかし detached window の OS viewport / HWND は、active / passive の表示状態をまたいで window id に紐づく。

そのため、次のような runtime state が必要かもしれない。

```text
DetachedWindowRuntime {
    id,
    viewport_id,
    last_hwnd,
    hwnd_generation_or_seen_epoch,
    placement,
    placement_seed_pending,
    last_outer_rect,
    role: Active | Passive,
}
```

この場合、active bundle はページ列 / cache / AI state を持つが、OS window lifetime は bundle ではなく
detached window runtime が持つ。

### 5.2 paused bundle へ退避するときに HWND 系 transient fields を保持すべきか

paused bundle をそのまま保持する設計は zoom / page / cache を保つために有効。
一方で、`detached_viewer_host_hwnd`, `fs_viewport_shown`, `fs_viewport_presentation`,
`fs_viewport_generation`, recreate flags など OS viewport transient fields まで bundle に入ると、
reactivation 時に stale 状態を復元する危険がある。

少なくとも pause 時または resume 時に、次を明示的に扱う必要がある。

- stale HWND は復元しない
- resume 後に active render が現在の stable viewport id の live HWND を再捕捉する
- 捕捉前に `host_lost_before_render` で recreate しない

### 5.3 passive viewport の初回 placement 適用は bool では足りない

`initial_placement_applied` は、viewport lifetime を識別する世代と紐づいていない。
bool のままでは、OS viewport が死んで再生成されたときに placement seed を再適用できない。

代替案:

- passive window の live HWND を追跡し、未知 / dead / changed の場合は placement を再 seed する。
- `show_viewport_immediate` で得た rect が default 800x600 相当かつ直前 placement と大きく違う場合、
  その frame の placement_update を採用しない。
- `initial_placement_applied` を `placement_seed_epoch` のようにし、viewport recreate を検出したら reset する。

### 5.4 close 後 focus 誘導は必要か

focus 誘導は active 化の連鎖を止めるものではなく、OS の見た目を安定させる補助である。
根本の viewport lifetime が安定すれば不要になる可能性がある。

現時点では `main_focus_after_close` の直後に default geometry 更新が観測されているため、
この補助処理が副作用を持つかどうかもレビュー対象にする。

## 6. 次に入れるべき追加ログ候補

修正前にさらに確認するなら、以下のログが有効。

- `activate_detached_image_window_snapshot()`:
  - snapshot id
  - snapshot placement
  - paused bundle の `detached_viewer_host_hwnd`
  - paused bundle の `fs_viewport_shown`, `fs_viewport_presentation`
- `pause_current_active_detached_viewer_context()`:
  - snapshot id
  - active の current `detached_viewer_host_hwnd`
  - captured placement
- `render_detached_image_windows()`:
  - `apply_initial_placement`
  - `initial_placement_applied`
  - outer / inner rect
  - placement update を採用したか / default geometry として捨てたか
- `host_lost_before_render`:
  - `detached_viewer_window_id`
  - stale hwnd
  - active bundle がどの snapshot id から復帰したものか

## 7. 現時点の暫定結論

今回の症状は、個別の focus / close / drag handler の問題というより、
「detached window の OS viewport lifetime をどこが所有するか」が曖昧なことによる状態不整合に見える。

特に次の 3 点が濃厚。

1. `ViewerContextBundle` に HWND / viewport transient state を含めたまま pause / resume しているため、
   reactivation 時に stale HWND を active context へ戻してしまう。
2. active window を passive snapshot 化するとき、placement を live OS rect からではなく
   `settings.detached_viewer_window_placement` / default から取る経路があり、pause 時点で
   snapshot placement が実 window geometry とずれる。
3. passive snapshot は stable viewport id を持つが、その OS viewport の生存を追跡しないため、
   viewport が再生成されたときに placement を再 seed できず、default geometry を正として保存してしまう。

ClaudeCode には、この理解が正しいか、また修正方向として
「per detached window runtime state へ HWND / placement seed / viewport lifetime を集約する」方針が妥当かを
レビューしてもらいたい。
