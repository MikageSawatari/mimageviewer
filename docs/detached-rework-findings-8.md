# 検収所見 #8: ON モードのクリック取りこぼし (armed 条件) + open 時フラッシュの証拠収集

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)
CUT fix3 (500cbd1f) / findings-7 (519f8faa) は検収合格 (テスト確認済み)。
本書はゲート C smoke 続行中 (2026-07-07) の新規 2 件。

## A1: passive 窓のクリックが飲まれることがある (アクティブ化の取りこぼし)

### 実機症状

ON モードで複数窓を切り替え中、窓をクリックしてもアクティブにならずホイールも
無効。何度かやると動く。

### ログ + コードで確定した機構

- ログ (bug-20260707-on-activate-miss.log): `passive_activate_queued` は 3 回だけで、
  **queued された 3 回は全て commit 成功**。= クリックが検知されれば動く。
  取りこぼしは **queue 以前**で起きている。
- コード ([ui_fullscreen.rs:3933](../src/ui_fullscreen.rs)):

  ```rust
  if !focused {
      window.activation_armed = true;   // 非フォーカスを 1 フレーム観測して初めて armed
  }
  ```

  activation は `can_activate && armed && user_activation`。snapshot 生成直後は
  armed=false で、**その窓が「フォーカスを失った状態」を一度観測するまでクリックが
  全て無視される**。park 直後の窓は OS フォーカスを保持したままのことがあり
  (クリック自体がフォーカスを与える)、その間クリックが飲まれ続ける。
  別の窓を触るなどでフォーカスが離れると armed が立ち、次のクリックから効く —
  「何度かやると動く」と一致。

### 修正要件

- armed の意図 (直前まで active だった窓への残クリック誤爆防止) を、フォーカス
  観測ではなく**状態ベース**で実現する。例: park/snapshot 生成の**次の root frame
  以降のポインタ press→release 完結**を復帰トリガにする (ParkedLive native 経路の
  press→release 変換と同じ意味論)。時間窓は禁止 (憲法 5)。
- 「park された窓はフォーカスの有無に関係なく、次フレーム以降のクリック 1 回で
  必ず復帰できる」をテストで固定 (focused=true のままの snapshot に対する
  pointer_activation → queued)。
- 誤爆防止の回帰: park を引き起こしたクリック自体 (同フレーム) では復帰しない。

## A2: ON モードで窓を開くときのフラッシュ (F12 と同種)

### 状況

- CUT fix3 (backstop 初回 host 生成の抑止) は**本ビルドで有効に動作している**
  (ログ: `keepalive_backstop skip` ×4、registered host は全て label=active_render、
  host_lost_diag 0 件)。**それでもフラッシュは残っている** → fix3 が塞いだ
  「短命 backstop host」説とは別の機構が存在する。
- 次の有力候補 (Fable): **新規 OS 窓が内容テクスチャの準備前に可視化され、
  クリア色 (ライトテーマ = 白) のフレームが見える**。前回の動画解析 (frame 247/317)
  で見えた「タイトルバー付き・全面白のフレーム」とも整合する。

### 進め方 (証拠を先に)

1. Codex: open 経路の可視化タイミングを調査 — 新規 detached 窓の
   `ViewportBuilder.with_visible` / Visible コマンド発行と、初回の内容描画
   (テクスチャ ready) の順序を確認し、`MIV_DETACHED_WINDOW_DEBUG=1` に
   「窓可視化時点で内容 ready だったか」のログを追加。
2. ユーザー: ON モードで窓 open を数回、録画 + ログ退避 (1 回)。
3. フラッシュしている窓の正体 (新規窓のクリア色フレーム / 他) をフレームと
   ログで確定してから修正案を Fable に出す。
   - 候補: 内容 ready まで窓を非可視のまま保つ (`Visible(true)` を初回内容描画後に
     送る)。読み込みが長い場合はローディング表示 (暗色) を先に描いて可視化。

## 完了条件

- [ ] A1: 修正 + 上記テスト。コミットに `(detached-rework findings-8 A1)` を含める
- [ ] A2: 調査ログ追加 (+ 望ましくは根因確定)。修正は Fable 承認後
- [ ] 既存テスト + full test 緑

---

## A1-v2 (2026-07-07): A1 修正後も実機 NG — 真因は egui deferred viewport への
ポインタイベント配送の欠落

### 実機ログの事実 (bug-20260707-a1-still-miss.log、Fable 解析)

- ユーザーが窓 1 をクリックし続けても、deferred callback の観測は
  **`pointer_pressed=false / pointer_released=false` が全パスで false のまま**
  (armed も永久 false)。一方 **focused=true / focus_edge は正しく届いている**。
- A1 前のコードも同じ `i.pointer.any_pressed()` 読みだった (diff 確認済み)。
  前セッションで 3 回成功していたのは配送が間欠的に届いた分で、**A1 のゲート
  変更は無関係。真因は「deferred viewport の egui 入力にマウスイベントが
  乗らない (少なくとも間欠的に欠落する)」**。
- 付随観測: 同一ミリ秒に同一 passive_event が 12 連続 (deferred callback が
  一瞬に 12 パス実行) — repaint 要求の暴走気味も併発。原因特定の材料にする。

### 対応方針 (Fable 指示)

**方針転換: アクティブ化のトリガを egui の deferred 入力に依存させない。**
フォーカスイベントは確実に届いているので、OS レベルの信頼できる信号で組む
(stale-F12 / ParkedLive native click と同じ「OS の物理状態を正とする」パターン):

1. **クリック復帰の新実装**: passive 窓への **focus 到達 (focus_edge) 時に
   `GetAsyncKeyState(VK_LBUTTON)` で物理左ボタンが押されている**こと =
   「クリックによるフォーカス」判定。その後の**ボタン物理解放** (root 側で
   ポーリング可能) で復帰を commit する。
   - Alt+Tab によるフォーカスは物理ボタンが上がっているので誤発火しない
     (クリック限定ルール維持)。
   - タイトルバードラッグでの移動と区別するため、press〜release 間の
     カーソル移動が小さいことを条件にしてよい (クリック/ドラッグ判別は標準的
     UI 慣行として許容)。
2. **egui 側の調査は並行で 1 ラウンドだけ**: egui-winit 0.33 のソースで deferred
   viewport への pointer イベント配送経路を確認し、「なぜ届かない/間欠なのか」
   「12 連続パスの原因」を報告する (将来 egui 更新時の判断材料。修正はしない)。
3. 既存の deferred 入力ベースの activation 経路 (press/release 観測) は撤去し、
   イベントキューには focus / close / placement 系だけを残す。
4. テスト: focus_edge + 物理ボタン押下 (cfg(test) 注入) → 復帰 commit /
   Alt+Tab 相当 (focus_edge + ボタン上げ) → 復帰しない / ドラッグ相当
   (大きなカーソル移動) → 復帰しない。
5. コミットに `(detached-rework findings-8 A1v2)` を含める。

### A1-v2 実装メモ / egui 0.33 ソース確認 (Codex, 2026-07-07)

- `egui-0.33.3/src/viewport.rs` の viewport 概説では、deferred viewport は
  「integration が後で、場合によって複数回 callback を呼ぶ」独立 repaint 型であり、
  通信は channel / `Arc<Mutex>` 前提と説明されている。
- `egui-0.33.3/src/context.rs:3891` 付近の `show_viewport_deferred` は、
  `viewport_ui_cb` を `ctx.viewports[new_viewport_id].viewport_ui_cb` に登録するだけで、
  callback 実行タイミングや入力配送は eframe integration 側に委ねている。
- `eframe-0.33.3/src/native/wgpu_integration.rs:542` 付近では root / deferred
  viewport 共通の `run_ui_and_paint` があり、deferred viewport は
  `egui_winit.take_egui_input(window)` でその window の raw input を取ってから
  登録済み `viewport_ui_cb` を実行する。したがって「focus は viewport info として
  届くが、press/release が常に届く」ことは mIV 側では保証できない。
- 同ファイル `CloseRequested` 処理では viewport と parent の両方に
  `request_repaint_of` を投げており、`egui::Context::show_viewport_deferred` の
  docs も callback が複数回呼ばれうることを明記している。mIV 側で focus-only /
  pointer-only のたびに root repaint を要求すると、今回観測した 12 連続 pass を
  増幅し得るため、A1-v2 では `focused && physical_left_button_down` のときだけ
  root repaint を要求し、focus-only では要求しない。
- 修正方針: deferred callback の `i.pointer.any_pressed/released` は activation
  入力として使わず、focus edge + `GetAsyncKeyState(VK_LBUTTON)` + `GetCursorPos`
  で物理クリックを開始し、root pass の物理解放で commit する。press-release 間の
  カーソル移動が 8px を超えた場合はドラッグとして破棄する。
