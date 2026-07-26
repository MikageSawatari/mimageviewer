# Codex 検収 (2026-07-09 深夜バッチ: クリック照準 / トースト面 / Image descriptor / §1.7)

## 初回レビュー

No P1 findings found.

[P2] [src/app.rs](C:/home/mimageviewer/src/app.rs:27423) adds a UI-thread folder scan during parked still reactivation. The new `Image` descriptor fallback calls `app.load_folder(parent)` from the activation path; `load_folder` is documented as synchronously running `scan_directory` on the UI thread at [src/app.rs](C:/home/mimageviewer/src/app.rs:12369), and `load_folder_with_scan` falls back to `scan_directory_with_settings` without a pre-scan at [src/app.rs](C:/home/mimageviewer/src/app.rs:12790). That means clicking a parked still whose stamp no longer resolves can freeze the UI on large or network folders. [src/app.rs](C:/home/mimageviewer/src/app.rs:27739) also adds a `Path::is_dir()` UI-thread filesystem check on the same path. This should go through an async/pending reopen flow that keeps the parked snapshot until the folder scan result is ready.

[P3] [src/tag_ops.rs](C:/home/mimageviewer/src/tag_ops.rs:466) can leave `tag_toast_surface` stale for no-op clear requests. `request_tag_clear_for_selection` sets `tag_toast_label = None` and `tag_toast_surface = Some(surface)` before it knows there are targets; if `count == 0` returns at [src/tag_ops.rs](C:/home/mimageviewer/src/tag_ops.rs:471), the next in-flight tag completion can be routed using the wrong surface. Move the label/surface assignment below the empty-target and precheck exits, matching the toggle/add paths.

[P3] [src/tag_ops.rs](C:/home/mimageviewer/src/tag_ops.rs:721) does not consume `tag_toast_surface` when a tag batch finishes with errors. The success branch takes both label and surface at [src/tag_ops.rs](C:/home/mimageviewer/src/tag_ops.rs:726), but the error branch only reads `self.tag_toast_surface`, so a failed completed batch leaves stale routing state for later tag feedback. On the final error poll, clear or take the surface/label lifecycle state the same way the success path does.

I did not run tests or builds; the sandbox is read-only.
## P3 修正後の再検収 (resume)

Clean on the two P3 fixes.

`request_tag_clear_for_selection` now leaves `tag_toast_label` / `tag_toast_surface` untouched until after empty-target and `precheck_tag_write_available` exits pass (`src/tag_ops.rs:466-486`). That closes the stale overwrite path.

`poll_tag_write_results` now keeps surface state for partial-batch errors, but clears label/surface unconditionally once `just_completed` is true (`src/tag_ops.rs:709-743`). That is the right lifecycle: error-completed batches no longer leak routing state, and in-progress batches still retain it.

P2 acceptance rationale looks sound. I do not see a path where the Image fallback fires repeatedly without explicit activation. The Windows watcher queues activation from a left-button release, then consumes one pending id before commit; the egui passive path requires pointer press + release via `detached_passive_window_update_activation`. Missing parent returns the snapshot to parked state without an internal retry loop. So the sync `load_folder(parent)` remains a real UI-hitch tradeoff, but I don’t see a worse background/repaint loop hidden behind it.

Verified:
`cargo test tag_targets_resolve_by_action_surface`
`cargo test feedback_toast_surface_is_recorded_per_origin`
`cargo test parked_still_reopen_descriptor_adds_image_fallback_without_touching_open_routing`

One housekeeping note: my temporary `rustc` probe created `rmeta4XjAM8/full.rmeta`; the restricted token can’t delete it despite it being untracked. It should be removed before commit.
## 第3状態 横断監査 (新セッション、§1.7 出荷前)

**Findings**

[P1] [src/app.rs](C:/home/mimageviewer/src/app.rs:28596)  
`detached_independent_session_blocks_folder_nav()` still treats “detached media window” as blocked only when `detached_viewer_open_images_in_window == true`. In the new third state, media opens in a detached window but this guard returns false unless an active context is already parked. As a result, Ctrl+Up/Down and Ctrl+PageUp/PageDown from the detached media window can start folder navigation instead of no-oping as detached windows should. This flows through [src/ui_fullscreen.rs](C:/home/mimageviewer/src/ui_fullscreen.rs:12463), [src/ui_fullscreen.rs](C:/home/mimageviewer/src/ui_fullscreen.rs:12560), native video handling at [src/app/native_video.rs](C:/home/mimageviewer/src/app/native_video.rs:6243), and gamepad handling at [src/app/gamepad_input.rs](C:/home/mimageviewer/src/app/gamepad_input.rs:4122). The resulting folder-nav apply path closes/reopens fullscreen at [src/app.rs](C:/home/mimageviewer/src/app.rs:24295), so this can unexpectedly replace/tear down the media session.

[P2] [src/app.rs](C:/home/mimageviewer/src/app.rs:28463)  
`should_promote_active_detached_video_for_main_context_change()` preserves only `GridItem::Video`. In third-state media-window mode, audio is also routed to the detached media window, but this predicate returns false for `GridItem::Audio`, so callers fall through to `close_fullscreen()` during main context changes such as drive list entry and folder loads at [src/app.rs](C:/home/mimageviewer/src/app.rs:12396) and [src/app.rs](C:/home/mimageviewer/src/app.rs:16804). That means detached audio playback is stopped instead of being parked/preserved while the main window browses images, despite the new preference describing both video and audio as independent media-window playback.

[P3] [src/ui_dialogs/first_setup.rs](C:/home/mimageviewer/src/ui_dialogs/first_setup.rs:69)  
The first-run dialog still presents only the old two-mode choice: full-feature window vs multiple windows. It has no control or explanation for `settings.fullfeature_media_window`, while preferences already expose the new checkbox at [src/ui_dialogs/preferences/pages.rs](C:/home/mimageviewer/src/ui_dialogs/preferences/pages.rs:127). New users cannot choose or discover the third state during initial setup, so the setup summary is now incomplete.
## audit P1/P2 修正後の再検収 (resume)

指摘なし。P1/P2 の修正は、第三状態・通常フル機能・複数ウィンドウの分岐として整合しています。

確認点:
- [src/app.rs](C:/home/mimageviewer/src/app.rs:28599) の folder nav ブロックは `effective_media_in_media_window()` + detached + media に限定されていて、checkbox OFF の通常フル機能や F12 連動画像には波及していません。
- [src/app.rs](C:/home/mimageviewer/src/app.rs:28463) の promote Audio 拡張は妥当です。Audio は `FsCacheEntry::Video` として再生状態を持ち、bundle swap 対象にも入っているため、`native_video_in_window_active = false` は Audio では副作用になりません。
- 3 つの main context change call site は、promote 成功時に still preserve / `close_fullscreen()` を飛ばす順序のままで問題ありません。画像は `viewer_item_is_media()` false なので既存 still preserve 経路に残ります。

実行確認: `cargo test fullfeature_media_window_blocks_folder_nav_and_promotes_audio` は対象テスト 1 件 pass です。
## 発火面フォローアップ (2026-07-10: スタック gate / コンテナ★反証 / Delete キー)

### スタック gate + 退避 wrapper
[P3] [src/filename_stack_ui.rs](C:/home/mimageviewer/src/filename_stack_ui.rs:293)  
`TryRecvError::Disconnected` still applies the fallback result without running the new detached-session evacuation. The `Ok` path receives, calls `park_detached_session_for_stack_aggregation()`, hard-closes on failure, then applies. But the disconnected path takes `stack_script_pending` and calls `apply_stack_script_result(...)` immediately. If the worker spawn fails or the worker drops `tx` while an unbundled detached session is active, `start_loading_items()` can still swap `items` under a live `fullscreen_idx`, which is exactly the dangling-session case the new flow is meant to avoid. The disconnected fallback should share the same post-recv evacuation path before applying.

The rest looks clean:
- `still_valid` semantics are unchanged.
- The evacuation only fires after `try_recv` returns a terminal result, not while the script is still computing.
- `stack_script_pending` is not in `ViewerContextBundle`, so the preserve-main-context clone path does not strand or duplicate it.
- The plain/no-window flow still goes straight through to `apply_stack_script_result`.

Tests run:
- `cargo test stack_aggregation_park_keeps_media_playing_and_main_folder`
- `cargo test container_rating_in_bundled_media_window_targets_window_folder`

Both pass.
### P3 (Disconnected fallback) 修正後
Clean. `Ok` and `Disconnected` now both produce `received` and fall through to the shared post-recv path:

- `Empty` still returns immediately at [src/filename_stack_ui.rs](C:/home/mimageviewer/src/filename_stack_ui.rs:292), before evacuation or pending take.
- `Disconnected` now returns fallback `Err(...)` at [src/filename_stack_ui.rs](C:/home/mimageviewer/src/filename_stack_ui.rs:296), then shares evacuation, hard-close guard, `take()`, and `apply_stack_script_result()` with `Ok`.
- The previous bypass is gone.

Verification: `cargo test stack_aggregation_park_keeps_media_playing_and_main_folder` passed.
## F12/リング メディア切替の案内トースト (2026-07-10 実機 FB)

[P3] [src/app.rs](C:/home/mimageviewer/src/app.rs:42424): video main-direction toast can fire even when the video placement switch is a no-op. `switch_native_video_viewer_presentation` can return early when tile mode is active, the video player/cache is not ready, or target rect resolution fails ([src/app/native_video.rs](C:/home/mimageviewer/src/app/native_video.rs:2125)). The always-new media branch then still emits “メインウィンドウ表示に切り替えました...” at [src/app.rs](C:/home/mimageviewer/src/app.rs:42444), although `viewer_presentation` may still be `DetachedWindow` and no setting changed. Audio/egui migration does not have this problem because `apply_egui_viewer_presentation_for_current_item` applies synchronously.

Everything else checked clean: the toast is inside `target != current` and non-detached gating, all production F12/ring/HUD/gamepad paths funnel through `toggle_detached_viewer_mode`, no other production caller of `always_new_media_f12_target_presentation` emits feedback, and native overlay linger passes the requested duration through.

Verification: `cargo fmt --check` passed. Targeted `cargo test` / `cargo check` could not complete in this environment due Windows pagefile/access-denied failures in `target`/target-dir creation, not a Rust diagnostic from this diff.
### P3 (no-op ゲート) 修正後

Clean.

`switch_started` gating is correct in the diff:

- Native video no-op paths leave `native_video_mode_switch` unchanged, so no toast.
- A real native placement request writes a new pending request id, so the main-direction hint fires.
- Expired pending is handled: the outer in-flight gate lets it through, `switch_native_video_viewer_presentation` replaces it after incrementing `native_video_mode_switch_seq`, so `request_after != request_before`.
- Egui/audio path is synchronous and sets `switch_started = true`.
- Detached-direction remains silent because the toast still requires non-`DetachedWindow`.

Checks run: `git diff --check -- src/app.rs src/app/tests.rs` and `cargo fmt --check`, both passed.