# Detached Window Phase A2 Runtime Separation

作成日: 2026-06-28

目的: Phase A1 の棚卸しに従い、active detached viewport の OS window runtime を
`ViewerContextBundle` から分離する。これにより、paused bundle から stale HWND /
viewport shown state / focus transient を resume 時に復元しない。

前提:

- Phase A1 棚卸し: [detached-window-phase-a1-transient-audit.md](detached-window-phase-a1-transient-audit.md)
- 症状調査: [detached-window-current-behavior-investigation.md](detached-window-current-behavior-investigation.md)

## 1. 実装方針

Phase A2 では per-window runtime map までは作らず、active detached viewport は常に 1 つである前提で
既存 App 直持ち field を active viewport runtime として扱う。

`ViewerContextBundle` は page / cache / AI / editing / reading state だけを持つ。
次の viewport / HWND / focus transient は bundle から外した。

- `detached_viewer_host_hwnd`
- `fs_viewport_shown`
- `fs_viewport_presentation`
- `fs_viewport_generation`
- `fs_viewport_recreate_after_hide`
- `detached_viewer_recreate_on_next_render`
- `fs_viewport_virtual_desktop_synced_hwnd`
- `detached_viewer_focus_requested`
- `detached_viewer_no_activate_once`
- `still_fullscreen_viewport_enter_suppress_until`
- `fs_opened_at`
- `fs_focus_grace_elapsed`
- `fs_prev_focused`
- `fs_focus_regained_at`
- `fs_prev_foreground_hwnd`
- `fs_last_native_focus_claim_at`
- `fs_last_main_focus_restore_at`
- `fs_suppress_primary_until_release`
- `detached_viewer_borderless_fullscreen`
- `detached_viewer_restore_placement`
- `detached_viewer_borderless_transition`

`detached_viewer_window_id` は bundle に残す。これは OS HWND ではなく、active / passive が同じ
`detached_image_window_viewport_id(id)` を導出するための stable identity である。

`pending_detached_video_host_switch` は動画 detached 経路の pending であり、今回の PDF / ZIP book viewer
問題とは別なので bundle に残した。

## 2. 新しい runtime helper

### 2.1 新規 active window

`reset_active_detached_viewport_runtime_for_new_window(generation, reason)` を追加した。

役割:

- active viewport は未表示として `fs_viewport_shown=false`
- `fs_viewport_presentation=None`
- `detached_viewer_host_hwnd=0`
- recreate / focus / native focus / borderless transient を reset
- `fs_viewport_generation=generation`

呼び出し箇所:

- `start_active_detached_book_context_from_descriptor()`
- `prepare_detached_image_windows_for_open()` で既存 active viewport を新 window 用へ切り替える場合

### 2.2 paused window の resume

`adopt_active_detached_viewport_runtime_from_passive(reason)` を追加した。

役割:

- passive として存在していた stable viewport を active として使うため、`fs_viewport_shown=true`
- `fs_viewport_presentation=Some(DetachedWindow)`
- stale HWND を復元せず `detached_viewer_host_hwnd=0`
- `detached_viewer_focus_requested=true`
- focus / native focus / recreate transient を reset

重要: `detached_viewer_host_lost()` は `host_hwnd == 0` の場合 false を返す。
そのため resume 直後に `host_lost_before_render` へ落ちず、次の active render 内で live viewport から
HWND を再捕捉する。

呼び出し箇所:

- `activate_detached_image_window_snapshot()` の `paused_bundle` resume 経路

## 3. pause helper の変更

`ViewerContextBundle::pause_background_work_keep_current_frame()` から以下の reset を削除した。

- `detached_viewer_focus_requested`
- `detached_viewer_recreate_on_next_render`
- `detached_viewer_no_activate_once`

これらは bundle field ではなく active viewport runtime field になったため、pause helper では扱わない。
slideshow / pending worker / seek / holdover / AI pending cancel など content 側の停止処理は維持する。

## 4. テスト変更

### 4.1 window id による viewport identity 検証

`detached_book_followup_open_uses_new_fullscreen_viewport_generation` は
`detached_book_followup_open_uses_new_detached_window_id` に変更した。

理由:

- detached window の ViewportId は `fs_viewport_generation` ではなく `detached_viewer_window_id` から導出する。
- `fs_viewport_generation` は A2 で bundle から外れた runtime state であり、book context identity ではない。

### 4.2 stale HWND 復元防止

`reactivating_paused_detached_book_reuses_bundle_without_reenumerating` に次の assert を追加した。

- resume 後 `fs_viewport_shown == true`
- `fs_viewport_presentation == Some(DetachedWindow)`
- `detached_viewer_host_hwnd == 0`
- `detached_viewer_host_lost() == false`
- `fs_viewport_recreate_after_hide == false`
- `detached_viewer_recreate_on_next_render == false`
- `detached_viewer_focus_requested == true`

これにより、paused bundle から stale HWND / recreate flags を復元しないことを固定する。

## 5. A2 でまだ解決しないこと

Phase A2 は Finding A (`host_lost_before_render` による recreate) の解消を狙う。

次は Phase B で扱う。

- pause snapshot の placement を live OS rect から取得する
- `initial_placement_applied` を viewport lifetime と結びつける
- default 800x600 相当 geometry の誤採用を防ぐ

そのため、A2 後も placement default 化の症状は残る可能性がある。

## 6. 検証

実施済み:

```text
cargo check --bin mimageviewer-core
cargo test --bin mimageviewer-core still_window_mode_key_tests
```

期待する実機ログ:

- paused window resume 直後に `active_viewport_runtime_adopt_passive ... old_host=...` が出る。
- その後 `recreate viewport: reason=host_lost_before_render` が出ない。
- active render 内で `captured host hwnd=...` が再度出る。

## 7. ClaudeCode レビュー依頼

確認してほしい点:

1. A1 で合意した 2.1 + 2.2 + borderless transient が bundle / `swap_field!` から抜けているか。
2. `detached_viewer_window_id` を bundle に残す判断が実装上も崩れていないか。
3. resume 経路で stale HWND / recreate flag を復元せず、`host_hwnd=0` から再捕捉する形になっているか。
4. 新規 active window 経路で runtime reset が不足していないか。
5. A2 として placement 問題を Phase B に残す切り分けが妥当か。
