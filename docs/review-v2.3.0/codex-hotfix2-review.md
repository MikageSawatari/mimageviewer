結論: クリーン。今回差分に P1/P2/P3 の指摘はありません。

`src/app.rs:27225` の legacy preserve 成功分岐で、`fullscreen_idx=None` / `viewer_presentation=non_detached` / `detached_viewer_window_id=None` まで落としているため、次回 `viewer_session_is_detached()` は成立せず、ゴースト idx による再 preserve は止まります。

呼び出し元との衝突も見当たりません。`activate_parked_live_media_window_snapshot` は cleanup 後に `src/app.rs:27015` 以降で window 61 を active session として明示的に立て直します。paused bundle / still snapshot / descriptor reopen 側も、それぞれ `src/app.rs:27497`、`src/app.rs:27568`、`src/app.rs:27635` 以降で次の active id または book context を再設定するので、前セッションの後始末とは噛み合っています。

`fs_viewport_shown` / host handoff の分担も問題ありません。legacy preserve 内の `src/app.rs:28279` が `handoff_active_detached_viewport_to_passive` を呼び、そこで `src/app.rs:25908` 以降の active viewport 状態を落としています。activation 後に `fs_viewport_shown=true` へ戻るのは新しい active media session 側なので、旧 main 画像の残骸レンダとは別です。

実行確認:
- `cargo test app::tests::still_window_mode_key_tests::parked_live_activation_clears_ghost_linked_session -- --exact`
- `cargo test app::tests::still_window_mode_key_tests::activating_parked_live_media_closes_linked_active_window_in_off_mode -- --exact`
- `git diff --check -- src/app.rs src/app/tests.rs`

すべて通過しています。