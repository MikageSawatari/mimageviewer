# 内部 Close と利用者 Close の取り違え — 修正ブリーフ

実装は Codex、検収は ClaudeCode。**着手前に
[docs/detached-rework-plan.md](../detached-rework-plan.md) §2 (憲法) を読むこと。**
リワークのステージ外からの構造的修正で、§2 の適用範囲どおり双方が合意済み
(経緯は [README.md](README.md) §10.7)。完了後は同プラン §11 に記録する。

## 直す不具合

動画をフルスクリーン ↔ 別ウィンドウで F12 連打すると、再生が止まって別ウィンドウが閉じる。
**v3.3.0 でも起きる。R-27 の lease 移譲で起きやすくなった。**

実ログで原因は確定済み (backlog §1.0f (a) に全文):

```
1545.452  transition=29 target=fullscreen action=Destroy ... viewport_command=Close  ← 自分で送る
1545.532  F12 → transition=30 target=detached phase=begin host=0x0
1545.564  [detached-viewer] show viewport: ... host=hwnd=0        ← 同じ ViewportId を再表示
1545.564  [detached-viewer] viewport close_requested: presentation=None host=hwnd=0  ← ★
1545.566  active-detached-session action=clear reason=handle_fullscreen_close_request
1545.575  presentation-transition id=30 target=DetachedWindow effect=Destroy hwnd=0x0
1545.594  [decoder-lifecycle] video-decode thread exit: live_count=0                 ← 再生停止
```

★ は `presentation=None` かつ `host=hwnd=0` — まだ窓が無い viewport なので利用者が
閉じられるはずがない。112ms 前に自分が送った Close が、再利用された同じ ViewportId の
新しい viewport に届いている。

## 合意した設計 (Codex の第三案)

**内部 teardown では `ViewportCommand::Close` を送らない。** viewport を desired set から
外して破棄する。**terminal になった `window_id` / `ViewportId` は再利用しない** —
再度 detached に入るときは新しい ID を割り当てる。**terminal 前の反転・folder navigation・
active↔passive など非終端の移譲では、従来どおり同じ ID を維持する。**

### なぜ他の案を採らないか (Codex の調査結果)

- **「送った Close を覚えて 1 回だけ消費する」案は不可**。egui 0.33.3 では内部の
  `ViewportCommand::Close` と OS の × / Alt+F4 が**同じ `ViewportEvent::Close` になる**。
  `close_requested()` はイベント列に Close が含まれるかだけを返し、送信元・incarnation・
  個数を保持しない。`{window_id, incarnation}` を持っても照合できず、内部 Close が
  未配達のまま利用者の × が来ると**その × を誤消費し得る**。利用者 close の保証を満たせない。
- **「incarnation を ViewportId に混ぜる」案も不可**。incarnation は HWND が得られた後の
  `set_hwnd` で採番されるのに、ViewportId は HWND を作る `show_viewport_immediate` より
  **前**に要る。循環している。事前採番するならそれは実質「新しい window_id / session
  generation」であり、同一論理 session 内の HWND 再生成まで ID 変更にすると R3 の
  安定 ID 方針に反する。

### 利用者の × をどう保証するか

**抑制ではなく、内部 Close を生成しないことで保証する。** live viewport に届く
`ViewportEvent::Close` は利用者 / OS 由来だけになるので、
[ui_fullscreen.rs](../../src/ui_fullscreen.rs) の

```rust
if !embedded && ctx.input(|i| i.viewport().close_requested()) { close_fs = true; }
```

は**無条件のまま維持する**。expected-close 判定も consume 処理もこの前に挟まない。
terminal 後は新しい ViewportId になるので、旧 ID に残ったイベントが新 viewport の入力に
なることもない。

### ViewportId を変えたとき何が新規になるか (Codex 調査)

新規: egui の viewport input/pass state と command/event queue、eframe の `ViewportInfo` と
`egui_winit::State`、新しい winit Window / HWND、WGPU surface と depth/MSAA attachment。
**共有されるもの**: Painter、GPU device/queue、renderer、texture / font atlas。
F12 ごとの全 GPU 初期化にはならない。

⚠ **placement は ViewportId から自動復元されない。** eframe の永続 `WindowSettings` は
root window 用で、detached child は builder に `position` / `inner_size` / `maximized` を
再設定する必要がある。`build_detached_viewer_viewport_builder` と runtime / settings の
placement を新しい ID へ引き継ぐこと。

## 触ってよい範囲

- [src/app.rs](../../src/app.rs) — `finish_active_detached_session_close`、
  `close_active_detached_session_exact`、`ensure_detached_viewer_window_id`、
  detached の programmatic Close を送る各 helper
- [src/app/native_video.rs](../../src/app/native_video.rs) —
  `execute_video_presentation_host_effect` の `DestroyHost`、
  `request_native_video_viewer_presentation`
- [src/ui_fullscreen.rs](../../src/ui_fullscreen.rs) — detached viewport の programmatic
  Close producer。**`close_requested()` の利用者 close 処理は変更しない。
  `detached_image_window_viewport_id(window_id)` も変更しない**
- [src/app/detached_window_manager.rs](../../src/app/detached_window_manager.rs) —
  必要なら「再利用可能な live lease」の純粋判定 helper
- [src/app/presentation_transition.rs](../../src/app/presentation_transition.rs) —
  原則 reducer 本体ではなく、terminal 前後の順序テスト
- [src/app/tests.rs](../../src/app/tests.rs)

⚠ **動画経路だけを見ないこと。** active/passive 変換など、他の detached 経路にも
「Close を送った直後に同じ ID を再利用する」箇所がある。**detached child の Close producer
を全部同じ原則で監査する**こと。

## 完了条件

1. 純粋状態遷移: terminal になった H → 即時 reopen で必ず J ≠ H。
2. reducer / effect: **terminal 前**の反転は H を移譲し、Close / Destroy を出さない
   (R-27 の lease 移譲を壊さない)。
3. output: programmatic teardown は Close command を出さず、H が output から消えて J が出る。
4. handler: stale H の Close は J や兄弟を閉じない。
5. handler: **current J の `ViewportEvent::Close` は必ず session / runtime を閉じる**
   (ここを壊すと「窓が閉じられない」というより悪い不具合になる)。
6. folder navigation、active↔passive、PDF/ZIP defer など**非終端経路では同じ ID**。
7. J に position / size / maximized / borderless が復元される。
8. `cargo test -p mimageviewer --lib` が緑。`cargo fmt` 済み。既存 detached テスト 104 本を
   削除・弱体化しない (規則 8)。
9. 憲法規則 3 (App に新しい bool / Option を足さない) と規則 5 (時間窓・retry で吸収しない)
   に反していないことを完了報告で明示する。
10. コミットメッセージに `(detached-rework close-identity)` を含める。

## 報告してほしいこと

- 監査した Close producer の一覧と、それぞれ terminal / 非終端のどちらに分類したか
- placement を新 ID へ引き継ぐためにどこを変えたか
- 実機 smoke の具体的手順 (F12 連打・再生継続・安定後の × 一回で確実に閉じること)
