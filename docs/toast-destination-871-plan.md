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

## 8. 独立設計レビュー r1 (Sol / xhigh、2026-11-23、a583320fe): REJECT、構造的修正への合意は DISAGREE

方向 (宛先なしの sentinel を無くす) は構造的だが、宛先の所有境界が未解決という判定。改訂が要る点:

- P1: `Viewer` は窓を一意にしない。メインの窓の中のビューアと、active な別ウィンドウのビューアは同時に描かれ得る
  (`src/app.rs:4256`・`:103020`、`src/app/tests.rs:85347`)。宛先は面ではなく既存の context / window の identity で持つ。
  passive / pinned の静的な snapshot は通知を描かない (`src/ui_fullscreen.rs:23937`)。
- P2: `current_input_surface()` は projection で答えが変わる (`active_detached_context_is_at_rest` と投影中の
  `fullscreen_idx` の混在、Building・読み込み中)。projection に依存しない表示 owner からの解決が要る。
- P2: native の転送先は発行時に mount されている context の player (`src/app/native_video.rs:15110`)。宛先の解決と
  egui / native の選択を 1 つの発行経路にする。
- P2: native だけに出すと、保存完了の通知の「保存先を開く」(`RevealPath`) が失われる / 前の通知に付く。
  text・宛先・action を 1 つの payload にする。
- P2: native の時間は通常 / 中央寄せで終点が違い、`Immediate(0.7)` (シーク境界) の例外がある。centered が分かる時点で
  期限を 1 つに確定し、不透明度と削除の両方に使う。
- P3: 直接書く所の一覧が不足 (キャプチャ保存 `src/app.rs:95353`、AI 診断)。両方に出すことを確かめている既存テスト
  (`src/app/tests.rs:40106`、`src/app/tests/notification_duration.rs:66`) を仕様変更として先に列挙する。

## 9. 分割の決定と v5.7.0 に入れる範囲 (2026-11-23)

利用者の決定 (2026-11-23「時間だけ先に入れる」): §1.871 を分ける。v5.7.0 には native の通知の時間だけを入れる。
宛先 (§3.1〜§3.3) は v5.7.0 の後に、r1 の指摘 (窓の identity・projection に依存しない解決・native の配送と action を
1 つの経路に) を受けて設計し直す。§3〜§7 の宛先の部分は r1 で退けられた版として残す。

### 9.1 native の時間 (v5.7.0)

今のコード (`src/feedback.rs` の `resolve_timing`、`src/video/native_presenter/render_core.rs`):

- native の通知には時計が 2 つある。外す時刻 = `native_linger` (`render_core.rs:12865`)、見えなくなる時刻 =
  `native_fade_end[centered]` (`native_opacity_at`)。見えている時間は 2 つの小さいほう。
- `native_fade_end` は `[1.8, 2.5] × 設定の倍率` の決め打ち。長い文 (`Text` で 20 字を超える) や長い指定
  (`Seconds(3.5)` など) は、1.8 秒 (中央寄せ 2.5 秒) で見えなくなり、`native_linger` まで見えないまま残る。
  §1.402 のテスト (`notification_duration_native_standard_preserves_legacy_linger_and_fade`) が
  「見えないまま残る」を旧式として固定している。
- 短い通知 (`native_linger` が決め打ちより短い) は、`native_linger` で外れる (フェードの途中か前に消える)。

変更 (利用者の推奨案 Q2-A「長い文だけ延ばす、短い文は今のまま」):

- `native_fade_end[c] = max(決め打ち[c] × 倍率, native_linger)`。式はこの 1 か所だけ変える。
  - `native_linger` が決め打ちより長い → 見えなくなる時刻 = 外す時刻。最後の 0.35 秒 × 倍率でフェードして外れる。
    見えないまま残る時間が無くなる。
  - `native_linger` が決め打ち以下 → 今と同じ (外す時刻も見え方も変わらない)。
- 外す時刻 (`native_linger`) は変えない。時計を 1 つにまとめると、短い通知 (`Text` の 1.2 秒) が 1.8 秒に延び、
  シーク境界の `Immediate(0.7)` が 0.7 秒で消えなくなるため (r1 の P2 指摘の「延長なし」を守る)。
- `Immediate(0.7)` (シーク境界、中央寄せ) と `Immediate(2.5)` (一覧の端の案内、中央寄せ) は、
  `native_linger` が決め打ち以下なので変わらない。`SurfaceSeconds { native: 1.8 }` (保存完了) も変わらない。
- 中央寄せかどうかは今と同じく描画時に `centered` で選ぶ。両方の値が上の式で決まる。
- egui の時間は変えない。

テスト: §1.402 のテストの「長い linger で見えないまま残る」期待を、「linger まで見えて最後にフェードする」に直す
(仕様変更)。短い linger・`Immediate(0.7)`・`Immediate(2.5)`・倍率 (×0.6 / ×1.5、警告とエラーの Short の例外) は
今の期待のまま通ること。`Text` の長い文で native の見える時間が egui の時間と同じになることを足す。

### 9.2 独立レビュー (Sol / xhigh、95a041aab): ACCEPT WITH CHANGES

- P2: native の定期描画は 250ms 間隔 (`src/video/mod.rs:8202`) で、通知の描画は次の描画を予約しない。
  fade 区間 (0.35 秒 × 倍率) を描画が飛ばすと、不透明のまま外れる場合がある (例: 短い設定の 27 字)。
  → **直さない (設計担当の判断)**。不透明のまま外れるのは、短い通知 (`native_linger` が fade の開始より前) では
  今もいつも起きている。動画の再生中は毎フレーム描くので fade は描かれ、飛ぶのは一時停止中だけ。
  fade の途中を描くための描画の予約は native presenter の描画の段取りの変更で、§9.1 の範囲を超える。
  見えている時間は今より長くなるだけで、読めなくなる退行ではない。必要ならバックログで扱う。
- 他の `FeedbackTiming`・倍率の例外・`Immediate(0.7)` / `Immediate(2.5)`・保存完了・再表示・音楽ビューは整合、
  テストの弱体化なし、文書 3 本は式と一致 (レビュー担当の確認)。
