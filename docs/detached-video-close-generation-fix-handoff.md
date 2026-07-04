# 引き継ぎ: detached 動画 close の世代タグ化（根本修正）

作成 2026-07-01。前セッションが ツール呼び出し不調のため /clear。この文書だけで作業再開できるように書いた。

## 0. 進め方の制約（重要）

- **ユーザーへの応答は日本語**（CLAUDE.md 準拠）。
- **フルコンパイルが 4 分以上**かかる。実機検証のペースを落とさないため **コンパイル往復を最小化**する。
  = 関連する編集を **全部まとめてから `cargo check --bin mimageviewer-core` を 1 回**。テストも
  一括で足してから 1 回で回す。小刻みに check しない。
- 動画 presenter は `docs/video-architecture.md`（native presenter 節）が必読。UI は
  `src/video/native_presenter/`（`ui_fullscreen.rs` ではない）。
- 作業ツリーに未コミットの `CLAUDE.md` / `docs/next-release-backlog.md` 変更あり（ユーザー/Codex 由来）。
  **巻き込まないこと**。今回の修正は独立コミットにする。pathspec commit 推奨。
- テストは `cargo test --bin mimageviewer-core <name>`（`--lib` では app::tests が走らない）。

## 1. タスク

detached（別ウィンドウ）動画まわりのバグが「実機検証のたびに新しいバグが出て収束しない」。
根本原因を特定済み。**根本修正 + 実機切り分け用ログ**を実装する、が今回のゴール。

## 2. 根本原因（確定）

**native の close イベントに世代タグが無い**のが構造的な穴。

- `PlacementSwitched` / `PlacementSwitchFailed` は `request_id` を持ち stale 判定できる
  （[src/app/native_video.rs](src/app/native_video.rs) の `handle_native_video_output_event`、2045〜2087 付近）。
- しかし `NativeVideoOutputEvent::CloseFullscreen`（unit variant）と
  `NativeVideoOutputEvent::Window(NativeVideoWindowEvent::CloseRequested)` は**世代を持たない**。
- placement switch はウィンドウを rebuild する（`switch_native_placement`、native_video.rs 1710 付近）ので、
  旧 HWND teardown 由来の close が新 presenter と区別できない。
- その回避として 71ff5f09 が **500ms の時間窓**（`native_video_ignore_close_until`）で close を握り潰す
  = band-aid。時間ベースなので racy：①stale close が 500ms より遅れれば素通り→動画が閉じる、
  ②switch/resize 直後 500ms 内の**正当な close（×/Alt+F4/overlay）も飲み込む**→閉じられない。

さらに d133d735 が presenter 側で host switch 時に `WM_QUIT` を drain（presenter ループ生存のため）。
これは別facet の対処で、`discard_pending_quit_messages_for_host_switch` は WM_QUIT 以外を
`DispatchMessageW` で捌くので switch 中に任意メッセージを処理する副作用の懸念もある（要確認）。

## 3. 確定した潜在バグ（実機前に潰す対象）

- **P1-1: `PlacementSwitched` を request_id 検証『前』に適用**
  [src/app/native_video.rs](src/app/native_video.rs) 2049-2050。`apply_video_presentation_switched(presentation)`
  を request_id 一致判定より前に無条件で呼ぶ。stale/mismatch でも `viewer_presentation`・
  `active_detached_session` の begin/end・500ms 抑止が先に走る。F12 連打・host 待ち・deadline 超過で
  「旧 switch の成功通知が新状態を巻き戻す」。→ **request_id 一致を先に判定し、一致時のみ apply**。
  （独立に安全に直せる。まずこれ。）

- **P1-2: 500ms 窓が正当な close を飲み込む（resize でも再アーム）**
  `native_video_close_suppressed_after_switch`（native_video.rs 1918 付近）は close の由来を問わず
  500ms 内なら破棄・再試行なし。`suppress_native_video_close_after_placement_switch`（1912 付近）は
  `apply_video_presentation_switched`（1863 付近、`native_video_mode_switch.is_some()` で発火）で armed。
  さらに `sync_detached_video_child_presenter_rect`（1730、detached 窓 resize）も
  `switch_native_placement` + `native_video_mode_switch` を立てる（1756-1759）→ **resize のたびに
  500ms 抑止が再アーム**。detached 動画を resize 中/直後に × で閉じられない。
  → 世代タグ化（下記 §4）で時間窓を撤去して解消。

- **P2-3: 動画 pending/session の状態が bundle と app-global に分裂**
  `pending_detached_video_host_switch` は `ViewerContextBundle` 内、`native_video_mode_switch` /
  `active_detached_session` は app-global。pending host switch に request/session/window id が無く、
  context swap 後に古い pending が別 mount 中 context で処理される余地。09a801d5（動画既定 detached 化）で
  経路が増えて踏みやすい。→ placement を状態機械（`Idle|WaitingForHost|Switching|Active` +
  `video_session_id`/`request_id`/`placement_generation`/`detached_window_id`）に寄せるのが本筋。
  今回は最低限、pending に request_id/window_id を持たせ context 不一致 pending を cancel する。

- **P2-4: `destroy_silent()` が旧 HWND event を識別不能**
  [src/video/native_window.rs](src/video/native_window.rs) 522 付近。`post_quit_on_destroy` を落とすだけで
  旧ウィンドウ由来 event に印を付けない。世代タグ化で app 側が弾けるようになれば緩和。

- **P3-5: `begin_active_detached_session` が closing 中 session を復活**
  [src/app.rs](src/app.rs) 22333-22348。`closing=false` を無条件セット。teardown 中の再 begin で
  session が蘇り、後続 `finish_active_detached_session_close` が stale 化する race の余地。
  → begin 時に「同一 window_id かつ closing 中」なら resurrection を避けるガードを検討。

## 4. 根本修正プラン（本丸 = close の世代タグ化）

時間窓を廃し、**因果（世代/HWND トークン）ベース**に置換する。

### 4.1 まず読むべき送信元（前セッションでトレース済み）

close の app への送信は presenter 側（`src/video/mod.rs`）の 3 箇所：
- `src/video/mod.rs` 1458: `Command::CloseFullscreen => NativeVideoOutputEvent::CloseFullscreen`（overlay ×）
- `src/video/mod.rs` 2947: presenter が `NativeVideoOutputEvent::CloseFullscreen` を send
- `src/video/mod.rs` 3270: presenter が `NativeVideoOutputEvent::Window(event.clone())` を forward
  （WM_CLOSE→`NativeVideoWindowEvent::CloseRequested` は native_window.rs 1135 付近、per-window
   `window_state(hwnd).event_tx`（native_window.rs 1120）で送出）

**最初のステップ**: 上記 mod.rs 2947 / 3270 / 1458 の周辺を読み、その send 地点で「現 presenter の
世代 or HWND」が取れるかを確認する（取れるはず。presenter 構造体が現 HWND を持っている）。

### 4.2 実装方針（案・低〜中リスク版）

1. **世代 or window token を close に載せる**
   - presenter 構造体に単調増加 `presenter_generation: u64`（host switch/window rebuild ごとに +1）を持たせる。
     もしくは現 HWND（`isize`/`u64`）を token として使う（HWND は再利用され得るので generation の方が堅い）。
   - `NativeVideoOutputEvent::CloseFullscreen { generation: u64 }` に変更、
     `NativeVideoWindowEvent::CloseRequested` にも `generation`（または HWND）を付与
     （または `Window` forward 時に presenter が現 generation で包む）。
   - 送信 3 箇所（mod.rs 1458 / 2947 / 3270）で現 generation を stamp。
2. **app 側で現世代のみ受理**
   - App に `native_video_committed_generation: u64` を持たせ、PlacementSwitched 成功（request_id 一致）で
     presenter が報告した generation に更新。
   - close ハンドラ（native_video.rs 743 / 2036 / 2239）で `event.generation < committed` なら
     `[native-video] reject stale close gen=... committed=...` をログして無視。一致なら close。
   - **500ms 窓を撤去**: `native_video_ignore_close_until`（app.rs 5685）と
     `suppress_native_video_close_after_placement_switch` / `native_video_close_suppressed_after_switch`
     の 3 呼び出し（71ff5f09 追加分）を削除。
3. **P1-1 の順序修正**: native_video.rs 2049-2050 を「request_id 一致を先に判定 → 一致時のみ
   `apply_video_presentation_switched`」に。stale は presentation/session/suppression を触らない
   （ただし『pending 無しだが presenter と収束』ケースは現 generation/hwnd 一致時のみ許容）。
4. **P2-3（最低限）**: `pending_detached_video_host_switch` に request_id/window_id を持たせ、
   mount 中 context と不一致なら破棄。
5. **P3-5**: begin ガード。

> 注意: generation を event enum に足すと、その enum の全 match 箇所（app 側 handler、テスト）を
> 直す必要がある。**列挙して一括で直してから 1 回コンパイル**。`NativeVideoOutputEvent` /
> `NativeVideoWindowEvent` を使う箇所を `git grep` で洗い出してからやる。

### 4.3 実機切り分け用ログ（今回必ず入れる）

各 close / placement イベントで最低限を出す：
- `request_id`, `presenter_generation`(or committed), `old_hwnd`, `new_hwnd`, `placement`,
  `detached_window_id`, `active_session {source, closing}`, close の `accepted|rejected + reason`。
- placement switch 開始/成功/失敗、host switch の drain、session begin/finish にも 1 行ずつ。
- 既存 `log_detached_image_window_debug` / `crate::logger::log` の作法に合わせる。

## 5. 検証（コンパイル 1 回で済ませる段取り）

1. 全編集を終える（§4.2 の 1〜5 + §4.3 ログ + 既存テストの enum match 追従 + 新テスト）。
2. `cargo check --bin mimageviewer-core` を 1 回。エラーが出たら**まとめて**直して再 check。
3. 通ったら `cargo test --bin mimageviewer-core native_video`（＋ `still_window_mode_key_tests`,
   `active_detached`）。既存 114+ が緑を維持。
4. 追加テスト案:
   - 「旧世代の CloseFullscreen/CloseRequested は無視、現世代の close は受理」を app レベルで検証。
   - 「switch/resize 直後でも現世代の × は必ず close する」（500ms 窓撤去の回帰ガード）。
   - P1-1: 「stale PlacementSwitched は presentation/session を変えない」。
5. `cargo fmt`（コミット前必須）。

## 6. コミット

- 独立コミットで。未コミットの CLAUDE.md / next-release-backlog.md は巻き込まない
  （`git commit -- <触ったファイル>` で pathspec commit）。
- メッセージ例: `Filter stale detached video close by presenter generation` /
  `Add native video placement/close diagnostics`。
- CLAUDE.md 指示: 「コミットして」と言われたらローカル master へ merge まで。push/PR は明示時のみ。

## 7. 直近コミット履歴（文脈）

```
d133d735 Keep native video presenter alive during host switch  ← Codex 最新（WM_QUIT drain、band-aid の別層）
71ff5f09 Ignore stale native video close after placement switch ← 500ms 窓（撤去対象）
09a801d5 Default videos to detached media window setting         ← 動画既定 detached、F12 で detached↔main
caf3b025 Add window minimize ring action
47727345 Add fullscreen mouse button action candidates
d241f6ef Route detached video polling through active context     ← 別バグ(P1)修正済み・妥当と確認済み
```

## 8. Codex 活用（任意）

fresh-eyes / 第二意見が要る局面では `codex exec --sandbox read-only -o <out> - < <promptfile>`
（stdin 経由・`< /dev/null` 不要、prompt を temp ファイルに書いて渡す）。往復は
`codex exec resume --last -o <out> - < <promptfile>`（`--sandbox` は付けない）。詳細は CLAUDE.md
「Codex CLI レビュー」節。前セッションの root-cause 分析結論は本文書 §2〜§4 に反映済み。

## 9. 補足（前セッションの確認済み事項）

- d241f6ef（detached 動画を active context の mount 内で poll_video、main 側は抑止）は**妥当**と確認済み。
  再レビュー不要。
- お気に入り上限 100 化（3feaa8ce）は問題なし。
- `should_poll_main_video_context` / `active_detached_viewer_context_contains_video` は正しく機能。
- 「active 動画 context + main 動画」共存は grid open 時の park_and_close で到達不能（starve しない）。
