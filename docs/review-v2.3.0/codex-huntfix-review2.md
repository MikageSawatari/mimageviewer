クリーン。残る指摘はありません。

今回の `swap_load_complex_with_bundle` は意図どおり、legacy ParkedLive park 後の main 側に実ロード複合体を戻し、parked 側を `empty()` 由来の空複合体にしています。これで前回指摘した `reload_queue=None` による main サムネ再投入停止と、早期 return/drop が main worker pool を巻き込む経路は解消しています。

activation 側の理解も問題ありません。復帰した active メディア文脈は動画/音声再生と `fs_pending` を bundle 内で持ち、サムネ worker pool は不要です。画像/PDF の fullscreen load も `start_fs_load` 側の個別 `fs_pending` cancel/channel で動き、通常のフォルダロードに入る場合は `start_loading_items` が新しいロード複合体を作り直します。

確認済み:
- `git diff --check -- src/app.rs src/app/tests.rs`
- `cargo fmt --check`
- `cargo test --bin mimageviewer-core live_media_park_creates_parked_live_snapshot_without_closing`
- `cargo test --bin mimageviewer-core bundle_swap_preserves_thumb_request_bookkeeping_and_load_complex`
- `cargo test --bin mimageviewer-core linked_live_media_restore_then_close_keeps_main_grid_context`
- `cargo test --bin mimageviewer-core activating_parked_live_media_restores_video_session`
- `cargo test --bin mimageviewer-core parked_live_video_audio_mode_uses_music_display_and_preserves_mode`