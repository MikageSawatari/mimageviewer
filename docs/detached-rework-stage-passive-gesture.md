# stage-passive-gesture — 非アクティブな別ウィンドウで右ドラッグを受理する
# (backlog §1.100 / §4.5)

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)。
**着手前に同書 §2 (憲法) を読むこと。**
症状・根本原因・利用者決定の正本: [next-release-backlog.md](next-release-backlog.md) §1.100。
所有権設計の正本: [briefs/detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md) §5。

ブランチ: `detached-rework`。コミットメッセージに `(detached-rework R2)` を含める。

---

## 1. 直すこと

複数ウィンドウモードで画像ウィンドウを複数開くと、**非アクティブなウィンドウ上の右ドラッグが
完全な no-op** になる。マウスジェスチャもリングショートカットも効かず、右ドラッグでは
ウィンドウのアクティブ化すら起きない。左クリックでアクティブ化した後なら効く。

**利用者決定 (2026-08-21)**: 「ジェスチャを認識し、ジェスチャされた場合は自動でアクティブ化した
上で、ジェスチャコマンドを実行する」。両方のウィンドウが見えている以上、ジェスチャが受理される
のが期待動作という判断。

したがって満たすべき不変条件は 2 つ:

- **右ドラッグの入力状態と発火先は、入力を開始した viewer window が所有する。**
  アクティブウィンドウを切り替えても別ウィンドウの状態に上書きされず、閉じたウィンドウの状態も残らない。
- **非アクティブなウィンドウで成立したジェスチャは、そのウィンドウをアクティブ化してから、
  そのウィンドウのコンテキストに対して実行される。**

## 2. なぜそうなっているか (確認済み)

1. 非アクティブな**静止画**ウィンドウは `show_viewport_deferred` で描かれ
   ([ui_fullscreen.rs:11622](../src/ui_fullscreen.rs:11622))、コールバックが root pass へ返すのは
   `DeferredDetachedImageWindowEvent::Frame { close 要求 / focused / placement / ppp }` だけ
   ([app.rs:462](../src/app.rs:462))。**ポインタ入力を一切拾っていない。**
2. アクティブ化を担う OS watcher は `VK_LBUTTON` しかサンプルしない
   ([detached_window_manager.rs:326](../src/app/detached_window_manager.rs:326))。
   静止画ウィンドウの egui 側にはアクティブ化経路が無い
   ([ui_fullscreen.rs:11175](../src/ui_fullscreen.rs:11175) は focus を記録するだけ)。
3. `MouseGestureState` ([ring_shortcut.rs:2543](../src/ring_shortcut.rs:2543)) が持つ識別情報は
   `RightDragContext` (Grid / ImageFullscreen / VideoFullscreen / EditMode) だけで、
   **どのウィンドウのものかを持たない**。状態は App に 1 個
   ([app.rs:10010](../src/app.rs:10010))。複数の画像ウィンドウはすべて `ImageFullscreen` なので
   区別できない。`update_mouse_gesture` の「root/grid pass は fullscreen ジェスチャの所有者では
   ない」という判定 ([gamepad_input.rs:775](../src/app/gamepad_input.rs:775)) も
   `context` 比較なので、兄弟ウィンドウ同士は見分けられない。

`ParkedLive` の動画 / 音声ウィンドウだけは `show_viewport_immediate` で描かれ
([ui_fullscreen.rs:11745](../src/ui_fullscreen.rs:11745))、`any_pressed` / `any_released` を集めて
**release でアクティブ化**する ([app.rs:39217](../src/app.rs:39217))。こちらは
「最初の右ドラッグがアクティブ化に消費される」状態にある。

## 3. やること

### 3.1 ジェスチャ状態に所有者を持たせる

`MouseGestureState` と `MouseFlickState` (リングショートカット、
[ring_shortcut.rs:2579](../src/ring_shortcut.rs:2579)) に所有者を足す。

```rust
/// 右ドラッグを開始した入力面。`context` (表示種別) と直交する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RightDragOwner {
    /// ルート viewport (メイングリッド、またはそこに乗ったフルスクリーン)。
    Root,
    /// 独立 detached ウィンドウ。値は window_id。
    DetachedWindow(u64),
}
```

- `start_mouse_gesture` / `start_mouse_ring_flick` は所有者を受け取る。
  既存の呼び出し元 ([ui_main.rs:12769](../src/ui_main.rs:12769) /
  [ui_main.rs:13318](../src/ui_main.rs:13318) / [ui_fullscreen.rs:22814](../src/ui_fullscreen.rs:22814) /
  [native_video.rs:9596](../src/app/native_video.rs:9596)) は `Root` を渡す
  (**アクティブな detached ウィンドウも、ルート pass が描いているので `Root`**)。
- `update_*` は `context` と**同じ厳しさで所有者も照合**する。
  所有者が違う pass は、他面のジェスチャを生かしたまま `None` を返す
  ([gamepad_input.rs:775](../src/app/gamepad_input.rs:775) と同じ規約)。
- **同時に走るジェスチャは常に 1 つ** (状態が 1 個なので構造的にそうなる)。
  この不変条件をテストで固定する。

⚠ App に新しいフィールドを足すのではなく、**既存の状態に identity を足す**。憲法 3 に適合する。

### 3.2 非アクティブウィンドウのポインタ列を root pass へ運ぶ

`DeferredDetachedImageWindowEvent::Frame` ([app.rs:462](../src/app.rs:462)) に、その frame の
右ボタンサンプルを足す。

- `secondary_pressed` / `secondary_down` / `secondary_released` / `pointer_pos: Option<Pos2>`
- 収集は `vp_ctx.input(...)` で、`ParkedLive` 側が既にやっているのと同じ形
  ([ui_fullscreen.rs:11785](../src/ui_fullscreen.rs:11785))
- ドラッグ中は `vp_ctx.request_repaint_of(egui::ViewportId::ROOT)` を出して root pass を回す
  (既に close / placement 変化で同じことをしている
  [ui_fullscreen.rs:11695](../src/ui_fullscreen.rs:11695))。
  **時間ではなく「右ボタンが押されている」という事実で判定する** (憲法 5)。

root 側は `queue_deferred_detached_image_window_event`
([ui_fullscreen.rs:11158](../src/ui_fullscreen.rs:11158)) でサンプルを受け、
所有者 `DetachedWindow(id)` として `start_*` / `update_*_with_pos` を駆動する。
`ParkedLive` の immediate 経路も同じ所有者で同じ reducer を通す
(現在の `any_pressed` / `any_released` だけの収集を、同じサンプル形へ揃える)。

### 3.3 成立したら「アクティブ化 → 実行」を型付きの順序で

現在 `DetachedWindowRuntime` は `pending_deferred_activation: bool`
([detached_window_manager.rs:432](../src/app/detached_window_manager.rs:432)) を持ち、
`take_pending_deferred_activation` ([detached_window_manager.rs:670](../src/app/detached_window_manager.rs:670))
が取り出している。**この bool を typed intent に置き換える** (R2b 残件の「散在 state の typed 集約」)。

```rust
/// アクティブ化要求と、アクティブ化後に実行すべきこと。
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DetachedActivationIntent {
    /// クリック等による通常のアクティブ化。
    Plain,
    /// 非アクティブ中に成立した右ドラッグ。アクティブ化の完了後に実行する。
    RunRightDrag(PendingRightDragCommand),
}
```

`PendingRightDragCommand` は成立したジェスチャ / リングの結果 (パターンまたは方向と
`RightDragContext`) を持つ。**コマンドはアクティブ化が実際に完了した後にだけ実行する。**
アクティブ化が失敗した / 対象ウィンドウが消えた場合は **理由付きでログを残して捨てる**
(silent fallback にしない、憲法の一般原則)。

実行時は、対象ウィンドウがアクティブになっているので
`with_active_detached_viewer_context` ([app.rs:16099](../src/app.rs:16099)) で
そのコンテキストをマウントして既存の `trigger_mouse_gesture_action` /
リング実行を呼ぶ。**アクティブ detached の window_id が intent の window_id と一致することを
確認してから実行する。** 一致しなければ実行せずログに残す。

⚠ **`with_viewer_context` (R2e の registry) は要らない。** コマンドはアクティブ化後に走るため、
マウントされていないコンテキストへ適用する必要が無い。

### 3.4 短い右クリックと右ドラッグ無効時

`RightDragMode::Disabled` のとき、および `MouseFlickOutcome::ShortTap` のときの扱いを
**明示的に決めてコメントに残す**。本ステージの既定は:

- `Disabled`: 非アクティブウィンドウ上の右クリックは**従来どおり何もしない**
  (ジェスチャ機能が切られているので、勝手にアクティブ化しない)
- `ShortTap`: ジェスチャ / リングが有効なとき、短い右クリックは
  **アクティブ化してから既存の短クリック動作** (`apply_viewer_short_right_click_action`
  [ui_fullscreen.rs:22799](../src/ui_fullscreen.rs:22799)) を実行する

これと違う判断をするなら、実装せずに報告して止まる。

## 4. スコープ外 (本ステージでやらない)

- **ジェスチャガイド / リングメニューの、非アクティブウィンドウ内での描画。**
  非アクティブ側は DTO 駆動なので、ガイドを出すには view にガイド情報を足す必要がある。
  これは後続 (stage-passive-gesture-2) に回す。本ステージでは、成立時にウィンドウが
  アクティブ化されて結果トーストがそこに出る、までを動かす。
- viewer context registry (R2e)、純粋 reducer / 合法遷移 (R2f)
- OS watcher に右ボタンを足すこと (**やらない**。右押下だけでウィンドウを前面化する挙動は
  利用者が選ばなかった案)
- placement / HWND / viewport ID / visibility 述語

## 5. 触ってはいけないもの

- `find_visible_thread_window_matching_rect*` (憲法 1)
- geometry 由来の recreate (憲法 2)
- App への新しい detached 用 bool / Option (憲法 3)
- placement の新しい保存先 (憲法 4)
- 時間窓での競合吸収 (憲法 5)。ドラッグ中の repaint は「右ボタンが押されている」事実で判定する
- 既存の detached テストの削除・弱体化 (憲法 8)。特に
  release でアクティブ化することを固定しているテスト ([app/tests.rs:32555](../src/app/tests.rs:32555))
  は **`ParkedLive` の左クリック経路の仕様なので残す**。右ドラッグ経路は別の入口として足す

## 6. 完了条件

1. `cargo test -p mimageviewer --lib` が緑。既存のジェスチャ / リング関連テスト (約 42 本) と
   detached 関連テスト (約 188 本) を削除・弱体化しない。
2. 新規テスト (最低これだけ):
   - 所有者が違う pass は他面のジェスチャを消さない (Root と DetachedWindow(id) の相互)
   - 3 枚以上の独立ウィンドウで、各ウィンドウのサンプルがそのウィンドウのジェスチャだけを進める
   - ジェスチャは同時に 1 つしか存在しない
   - 非アクティブウィンドウで成立したジェスチャが `RunRightDrag` intent を作り、
     **アクティブ化完了後に**実行される (順序をテストで固定する)
   - アクティブ化前に対象ウィンドウが閉じたら、コマンドは実行されず理由付きで捨てられる
   - 対象ウィンドウの window_id とアクティブ detached の window_id が食い違うときは実行しない
   - `pending_deferred_activation: bool` の既存挙動 (`Plain` intent) が変わらない
3. `cargo fmt` 済み。`python scripts/check_ui_glyphs.py` が 0 件 (UI 文言を触った場合)。
4. 完了報告に、`bool` → `DetachedActivationIntent` の置き換えで影響した箇所を file:line で列挙する。

## 7. 実機 smoke (利用者が実施)

1. 複数ウィンドウモードで独立ウィンドウを 3 枚開く
2. 一番古いウィンドウ上で**アクティブ化せずに**マウスジェスチャを引く
   → そのウィンドウがアクティブ化され、コマンドがそのウィンドウに対して実行される
3. 別のウィンドウでも同じことをする → 前のウィンドウの状態が残っていない
4. 最新のウィンドウを閉じて、先行ウィンドウでジェスチャが続けて効く
5. リングショートカットモードでも 2〜4
6. メイングリッドと、native 動画のジェスチャが従来どおり動く (退行確認)
7. `ParkedLive` の動画ウィンドウ (再生中に別ウィンドウをアクティブ化して作る) でも 2 が動く
8. 右ドラッグを「無効」に設定したとき、非アクティブウィンドウ上の右クリックで
   ウィンドウが前面化しないこと
