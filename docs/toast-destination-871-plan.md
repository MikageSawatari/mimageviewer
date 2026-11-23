# §1.871 宛先なしの通知を 1 つの窓に出す・native の通知の時間を文字数に合わせる (設計)

状態: 設計 (2026-11-23)。独立設計レビュー待ち。v5.7.0 候補。
発端: §1.843 のサブ PC 確認 (観測者はサブ PC 担当、ビルド 047b20c7b)。閲覧表示から F12 で別ウィンドウを出した状態で
AI から設定を変えると、同じ文の通知が本体の窓と別ウィンドウに 1 つずつ出た。通知は約 2 秒で消え、3 行の通知は
読み切るには短かった。

detached の経路に触れるので、CLAUDE.md「Detached viewer リワーク中のルール」と
[detached-rework-plan.md](detached-rework-plan.md) §2 (憲法) に従う。合意が取れたら同書 §11 に記録する。

## 1. 今のコード (読んだ範囲。実行時の挙動は未確認)

- 通知の入口は `App::publish_feedback_toast(text, surface: Option<ActionSurface>, kind, timing)` (`src/app.rs`)。
  宛先を渡さない入口 (`show_feedback_toast` / `_warning` / `_error` / `_result` / `_with_duration`) の呼び出し元は
  約 1,100 か所、宛先を渡す入口 (`*_on`) は約 30 か所。
- 状態は App に 1 組だけ: `fs_feedback_toast: Option<(String, Instant, f32)>`、`fs_feedback_toast_action`、
  `fs_feedback_toast_surface: Option<ActionSurface>`。viewer context の入れ替え (`swap_field!`) の対象ではない。
- `ActionSurface` は `MainWindow` と `Viewer` の 2 つ。
- 描画は `draw_feedback_toast(ui, rect, ctx, drawing_surface)` (`src/ui_fullscreen.rs`)。呼び出しはメインの一覧
  (`src/ui_main.rs`、4 か所、`MainWindow`) とビューアの共通 overlay (`viewer_chrome.rs` の
  `draw_viewer_shared_overlays`、`Viewer`)。`fs_feedback_toast_surface` が `Some(s)` なら `s` の面だけ、
  **`None` なら描いたすべての面**に出る。
- **2 つ出る原因**: 宛先なしは `None` のまま保存される。ビューアを別ウィンドウに出していると、メインの窓の一覧
  (`MainWindow`) と別ウィンドウ (`Viewer`) の両方が同じフレームで描くので、窓ごとに 1 つずつ出る。ビューアが
  メインの窓の中にあるときは一覧を描かないので 1 つ。
- 宛先なしは、さらに Windows では native (動画) の overlay にも出す (`publish_native_video_overlay_toast`)。
  宛先 `Viewer` で動画を native で表示しているときは native だけに出す (`native_only`)。
- **約 2 秒で消える原因**: `feedback::resolve_timing` (`src/feedback.rs`) は egui の時間を文字数で決める
  (`feedback_toast_duration`: 20 字まで 1.2 秒、1 字ごとに +0.12 秒、上限 5.5 秒。通知の時間の設定で ×0.6 / ×1.5)。
  native は残す時間 (`native_linger`) は文字数どおりだが、不透明度が `native_fade_end = [1.8, 2.5] × 設定の倍率`
  で決め打ちに 0 になる (`native_opacity_at`)。長い文でも native は 1.8 秒 (中央寄せは 2.5 秒) で見えなくなる。
  サブ PC で見えた約 2 秒が native 側だったかは突き合わせていない (未確認)。
- 「どの窓に入力を配るか」の持ち主はすでにある: `App::current_input_surface()` (`src/app/gamepad_input.rs`) →
  `DetachedWindowManager::resolve_input_surface` (`src/app/detached_window_manager.rs`)。
  - ビューアが無い → `MainWindow`。ビューアがメインの窓の中 → `Viewer`。
  - ビューアが別ウィンドウ → 前面の窓がメインなら `MainWindow`、mIV の別の窓なら `Viewer`、
    mIV が前面でなければ `last_input_surface` (直近にゲームパッドの入力を配った面、既定は `MainWindow`)。
  - 同じ持ち主の注記 (`dispatched_input_surface`) に「描画時に面を判定し直すと両方の窓に overlay が出る。
    配った側が記録した 1 つの答えを共有する」とある (2026-08-29 の実機報告)。今回の 2 つ出る件と同じ形。

## 2. 組み合わせを減らす手の検討 (CLAUDE.md「設計の簡素化」)

- **宛先を「両方」にする状態を無くす (採用)**。`fs_feedback_toast_surface == None` は「宛先なし = 描いた面すべて」の
  sentinel で、窓の数だけ通知が増える組み合わせの元。通知を出す時点で宛先を 1 つに決めて保存し、
  「両方」という状態を持たない。宛先は通知の状態の一部にする (下の §3.3)。
- **呼び出し元ごとに宛先を付ける案は採らない**。約 1,100 か所あり、ワーカーの完了・AI の変更など「操作の元の窓」が
  無い通知も多い。呼び出し元の分類が新しい組み合わせになる。
- **新しい「どの窓か」の判定を作らない (採用)**。入力の配り先の持ち主 (`current_input_surface`) をそのまま使う。
  通知の宛先と入力の配り先が別の事実を見ると、片方だけ直したときに食い違う (R-02 と同じ所有権割れ)。
- **描画時に毎フレーム判定しない (採用)**。出す時点で 1 回決める。描画時に判定すると、窓ごとの projection で
  答えが変わり両方に出る (上の注記)。通知が出ている間に前面を切り替えても、通知は元の窓に残る。
- **native と egui の時間を 1 つの規則にする**。native だけ別の決め打ちの時刻で消える組み合わせを無くす (§3.4)。

## 3. 設計

### 3.1 宛先の決め方

`publish_feedback_toast` で、宛先が渡されなければ `current_input_surface()` で 1 つに決める。渡されていれば
そのまま使う (今と同じ)。決めた後は、宛先ありの通知と同じ経路を通る。

| 状況 | 宛先 |
| --- | --- |
| ビューアが無い | メインの窓 |
| ビューアがメインの窓の中 | ビューア (メインの窓の中) |
| ビューアが別ウィンドウ、前面がメインの窓 | メインの窓 |
| ビューアが別ウィンドウ、前面が別ウィンドウ | 別ウィンドウ |
| ビューアが別ウィンドウ、mIV が前面でない (AI アプリを操作中など) | 入力の配り先の既定 (`last_input_surface`。直近にゲームパッドで操作した面、無ければメインの窓) |

- 最後の行は利用者の判断事項 (§5 Q1)。
- 利用者がその窓で操作した結果の通知 (★・タグ・ページ送りなど) は、操作中の窓が前面なのでその窓に出る。

### 3.2 native (動画) の overlay

- 宛先が `Viewer` で、ビューアが動画を native で表示している → native の overlay だけに出す (今の `native_only` と同じ)。
- それ以外 → egui だけ。宛先なしの通知を native と egui の両方に出す今の分岐は無くなる。
  - ビューアがメインの窓の中で動画を native 表示しているとき、今も egui の通知は native の面の下になり、
    見えているのは native 側と読める (未確認)。見え方は変わらない見込み。

### 3.3 状態

- `fs_feedback_toast_surface: Option<ActionSurface>` を無くし、宛先 (`ActionSurface`、Option ではない) を
  `fs_feedback_toast` の中身に入れる。通知があれば必ず宛先がある。「通知はあるが宛先が無い」状態を型で作れなくする。
  - 形は実装担当が選ぶ (4 要素の tuple か名前付きの struct)。テストの読み替えが少ないほうでよい。
- 直接書く 3 か所も宛先つきで書く:
  - ★の所属の確定で通知を持ち越す 2 か所 (`src/app.rs`。今も `Some(Viewer / MainWindow)` を決めて書いている)。
  - `src/pipeline_debug.rs` のデバッグ出力。`show_saved_output_feedback` の後に宛先を `None` に戻している。
    宛先は `show_saved_output_feedback` が決めたものを残す (None に戻す理由をコードで確かめ、理由が「両方に出す」
    以外なら報告して止まる)。
- `fs_feedback_toast_action` はそのまま (宛先と同じ通知に付く)。
- App に新しい bool / Option を足さない (憲法 3)。1 つ減る。

### 3.4 native の時間

`resolve_timing` の `native_fade_end` を、文字数で決めた時間 (`native_linger` と同じ値) に合わせる。
選び方は利用者の判断事項 (§5 Q2)。推奨案 (Q2-A):

- native の消える時刻 = max(文字数で決めた時間, 今の決め打ち 1.8 秒 / 中央寄せ 2.5 秒) × 設定の倍率。
  長い文は egui と同じだけ出る。短い文は今の長さを保つ (短くしない)。
- 消える前のフェード (0.35 秒 × 倍率) は今と同じ。
- `SurfaceSeconds { egui, native }` で native の秒数を明示している呼び出しは、その秒数で同じ規則にする。
- native の overlay を外す時刻 (`native_linger`) も同じ値にする (消えた後も残る・見える間に外れる、の食い違いを
  作らない)。

### 3.5 触れる範囲

- `src/app.rs` (`publish_feedback_toast`、持ち越し 2 か所、field)、`src/ui_fullscreen.rs` (`draw_feedback_toast`)、
  `src/pipeline_debug.rs`、`src/feedback.rs` (`resolve_timing`)、`src/app/gamepad_input.rs`
  (`current_input_surface` を通知からも呼べるよう可視性だけ変える)、テスト。
- `resolve_input_surface` と `last_input_surface` の中身は変えない。detached の述語・viewport の経路・
  rect 一致捕捉・placement は変えない。

## 4. 憲法 (detached-rework-plan §2) との照合

| 条 | 判断 |
| --- | --- |
| 1 rect 一致捕捉 | 触れない |
| 2 geometry 由来の recreate | 触れない |
| 3 App に detached 用の bool / Option を足さない | 足さない。`fs_feedback_toast_surface` の Option を 1 つ減らす |
| 4 placement | 触れない |
| 5 時間窓で競合を吸収しない | 時間窓は使わない。宛先は出す時点の入力の配り先 (前面の HWND と既存の記録) で決める |
| 6 実機の新症状にヒューリスティックを入れない | 根本原因 (「両方」の sentinel と、native だけ別の決め打ち時刻) を直す |
| 7 ついでに直さない | 長い通知が窓の左右へはみ出す件 (折り返さない描画) は別件。触れない |
| 8 detached テストを弱めない | 弱めない。宛先なしで両方の面に出ることを確かめているテストがあれば、仕様変更として列挙して確認を取る |

症状パッチではない理由: 2 つ出るのは「宛先なし = 描いた面すべて」という状態が窓の数だけ通知を増やすためで、
その状態を型から無くし、宛先を既存の入力の配り先の持ち主 1 つで決める。guard・delay・retry・repaint を足さない。

## 5. 利用者に決めてもらう点 (推奨を先頭に)

- **Q1 mIV が前面でないとき (AI アプリを操作中など) の宛先**
  - A (推奨): 入力の配り先の既定と同じ (直近にゲームパッドで操作した窓、無ければメインの窓)。判定が 1 か所のまま。
    マウス・キーボードだけで使っていると、別ウィンドウを出していてもメインの窓に出る。
  - B: ビューアを別ウィンドウに出していれば別ウィンドウ。AI の変更が見ている画像の窓に出るが、
    入力の配り先とは別の規則が 1 つ増える。
- **Q2 native (動画) の通知の時間**
  - A (推奨): 長い文だけ egui と同じに延ばす。短い文は今の 1.8 秒 / 2.5 秒のまま。
  - B: egui と完全に同じにする。短い文は今より短くなる (例: 20 字以下は 1.8 秒 → 1.2 秒)。
  - C: 変えない。

まれな失敗 (OS の前面の取得失敗など) は既定の割り切りで、`foreground_app_hwnd()` が `None` を返したときは
上の「mIV が前面でない」と同じ扱いになる (既存の持ち主の振る舞い)。

## 6. テスト

- 宛先なしの通知: ビューアなし → メインだけ。メインの窓の中のビューア → ビューア。別ウィンドウで前面がメイン /
  別ウィンドウ / mIV 以外 → それぞれ 1 つの面だけ (`fs_feedback_toast` の宛先と、両方の面で `draw_feedback_toast`
  を呼んだときに描くのが 1 つだけであること)。前面の切り替えはテスト用の `foreground_app_hwnd` を使う。
- 宛先なし + native の動画 → native だけ、egui の状態は作らない。宛先 MainWindow → native に出さない。
- ★の所属の持ち越し・デバッグ出力の通知が宛先つきで残る。
- `resolve_timing`: 長い文で native の消える時刻 = 文字数の時間 × 倍率、短い文は今の値 (Q2-A の場合)。
  既存の `notification_duration` のテストは Q2 の結果に合わせて期待値を直す。
- 既存の detached テスト (src/app/tests.rs) は変えない。赤くなるものは列挙して確認を取る。

## 7. 見積もり

設計レビュー 1〜2 往復、実装 (Codex Sol / high) 1〜2 時間、独立レビュー 1 往復の見込み。v5.7.0 に入る規模。
サブ PC の確認は、別ウィンドウ + AI の変更で通知が 1 つ、動画の通知が長い文で読み切れる、の 2 点。
