結論: クリーン。今回差分に P1/P2/P3 の指摘はありません。

確認ポイント:
- `src/app.rs:25737`: 直参照分岐にも `detached_image_windows` 衝突ガードが入り、passive / ParkedLive が所有中の `window_id` を新セッションへ再利用しない形になっています。
- `src/app.rs:25764`: folder-nav 用の `last_active_detached_window_id` 再利用にも従来どおり同じ衝突ガードがあり、今回の追加で linked / folder-nav の active 窓再利用は壊れていません。active 窓は `detached_image_windows` ではなく active session / active context 側にあり、passive 化・ParkedLive 化したものだけが `detached_image_windows.push` されます。
- `src/app.rs:28184`: `park_active_detached_context_as_live_media` は park 成功後に main 側へ戻った stale `detached_viewer_window_id` を消しており、今回の実害経路を発生源でも塞げています。
- 同種経路は確認済みです。legacy live-park は `cloned_main_viewer_context_after_detached_live_media_park` と `park_current_viewer_context_as_live_media_inner` 側で main id を落としています。通常 pause 経路で stale コピーが残っても、今回の `ensure` 側ガードで parked/passive id の再利用は拒否されます。

実行確認:
- `cargo test app::tests::still_window_mode_key_tests::ensure_window_id_rejects_id_of_parked_window -- --exact`
- `cargo test app::tests::still_window_mode_key_tests::park_active_media_context_clears_stale_main_window_id -- --exact`
- `cargo test app::tests::still_window_mode_key_tests::folder_nav_reopen_reuses_active_detached_window_id -- --exact`
- `cargo test app::tests::still_window_mode_key_tests::detached_folder_nav_reopen_reuses_window_even_if_grid_intent_returns -- --exact`
- `git diff --check -- src/app.rs src/app/tests.rs`

すべて通過しています。