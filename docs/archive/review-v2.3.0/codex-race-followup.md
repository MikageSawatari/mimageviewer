結論: 前回の P1 は撤回します。現行コードの「メディア窓 1 本規則」と閉窓時 discard をすり抜ける、ユーザー操作レベルの到達経路は見つかりませんでした。

1. 窓A pending → 窓B update は成立しません。

`ParkedLive` 由来の pending は、`poll_parked_live_detached_windows` 中に `native_video_parked_live_input_window_id = Some(A)` の状態で作られます。その pending が生きている間に別メディアを active で開くと、`open_fullscreen` → `prepare_viewer_presentation_open` → `close_parked_live_media_windows_for_new_media` → `remove_detached_window_runtime` → `discard_parked_source_swap_pending_for_window` が同期的に走り、窓Aの pending は破棄されます。

同じ UI thread 内の同期処理なので、「close 予定だが pending はまだ残っていて、その間に窓Bが update 分岐へ入る」というタイミング窓も見つかりません。`detached_image_windows.retain(...)` で A 自体も parked list から外れます。

非メディアを開く場合は `close_parked_live_media_windows_for_new_media` は走りませんが、その経路は video source-swap を起動しないため、既存 pending の update 分岐へ入りません。

2. active session の動画ナビが parked pending を update する経路も、通常操作では到達できません。

`update_active_detached_viewer_context` は `poll_parked_live_detached_windows` より前に走るため、同一フレームで「parked 側が pending を作った後に active 側が update」する順序はありません。次フレーム以降についても、active の新メディア open は parked media を閉じて pending を discard します。また main 側は `should_poll_main_video_context()` が parked live media 存在中に `poll_video` を止めます。

したがって `source_swap_owner_after_update(None, Some(B)) = None` が実害化するには、既に「active video と parked media pending が同時存在する」という 1 本規則違反の状態が必要です。通常のユーザー操作からは作れません。

[P3] 保守性リスクとして再分類  
場所: `src/app/native_video.rs:771`, `src/app/native_video.rs:803`, `src/app/native_video.rs:1048`, `src/app/tests.rs:23026`

`defer_native_video_source_swap_until_decoder_free` の update 分岐自体は、既存 pending の owner と現在 owner の一致をローカルに検証していません。`source_swap_owner_after_update(Some(8), Some(7)) == Some(8)` もテストで固定されており、この関数単体では「native_output は旧 owner の presenter のまま、owner だけ新 ownerへ移す」形を許します。

現状は外側の 1 本規則で防がれていますが、将来 `ParkedLive` メディアを複数許可する、または close/discard の順序を変える変更が入ると、この helper は静かに危険側へ倒れます。P1 ではなく、「この update 分岐は外部 invariant 依存である」と明示する P3 が妥当です。