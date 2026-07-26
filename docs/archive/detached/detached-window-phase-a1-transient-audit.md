# Detached Window Phase A1 Transient Audit

作成日: 2026-06-28

目的: detached book viewer の active / passive 切替で、`ViewerContextBundle` に入れるべき
状態と、OS viewport lifetime 側へ分離すべき transient state を棚卸しする。
この Phase では大きなコード変更は行わず、Phase A2 で `swap_field!` から外す対象と
影響範囲を確定する。

前提となる症状・ログ分析は
[detached-window-current-behavior-investigation.md](detached-window-current-behavior-investigation.md)
を参照。

## 1. 分類基準

`ViewerContextBundle` は「本 / ページ文脈」を pause / resume するための bundle とする。
つまり、次は bundle 所有のままでよい。

- page list / `items` / `current_folder`
- `fullscreen_idx`
- zoom / pan / spread / reading flow / continuous scroll
- page cache / AI cache / prefetch state
- editing state
- detached session の linked / independent / pinned など、読書 session の意味状態
- detached window identity (`detached_viewer_window_id`)

一方、次は bundle に入れない。

- OS viewport が表示中かどうか
- HWND
- viewport generation / recreate flag
- DWM / virtual desktop / focus claim / focus grace / primary suppression などの window 入力 transient
- borderless fullscreen transition など、OS window の見た目・placement に関わる runtime state

理由: active / passive を切り替えても OS window lifetime は `detached_image_window_viewport_id(id)` に
紐づく。content bundle と一緒に HWND / shown flag を swap すると、paused bundle が stale HWND を
保持し、resume 時に `host_lost_before_render` や default geometry 取り込みを起こす。

## 2. Phase A1 判定表

### 2.1 Phase A2 で bundle / swap から外すべきもの

| field | 現在の位置 | Phase A2 owner | 理由 / 参照影響 |
| --- | --- | --- | --- |
| `detached_viewer_host_hwnd` | `ViewerContextBundle` + `swap_field!` | active viewport runtime / per-window runtime | Win32 HWND は OS window lifetime。paused bundle に残すと resume 時に stale HWND を復元し、`detached_viewer_host_lost()` が `host_lost_before_render` を起こす。`capture_detached_viewer_host_hwnd_from_logical_rect`, `detached_viewer_host_lost`, `detached_viewer_window_placement` 周辺を runtime 参照へ寄せる必要がある。 |
| `fs_viewport_shown` | `ViewerContextBundle` + `swap_field!` | active viewport runtime | 「現在 active viewport を show 済みか」の flag。content bundle に入ると pause / resume 後に古い shown 状態を戻す。`render_fullscreen_viewport`, `keep_fullscreen_viewport_alive`, `finalize_closed_active_detached_viewport` の gating に影響。 |
| `fs_viewport_presentation` | `ViewerContextBundle` + `swap_field!` | active viewport runtime | `fs_viewport_shown` と同じ runtime tuple。active viewport が DetachedWindow / Fullscreen のどちらとして生きているかを示すため、bundle ではなく live viewport state が持つ。 |
| `fs_viewport_generation` | `ViewerContextBundle` + `swap_field!` | active viewport runtime | non-detached fullscreen の `ViewportId` 世代。detached book は `detached_viewer_window_id` 由来の stable id を使う。bundle に戻すと古い generation がログ / recreate と混ざる。 |
| `fs_viewport_recreate_after_hide` | `ViewerContextBundle` + `swap_field!` | active viewport runtime | hide 後に generation を進める transient。paused content が保持すべきではない。 |
| `detached_viewer_recreate_on_next_render` | `ViewerContextBundle` + `swap_field!` | active viewport runtime | 次 render で active detached viewport を recreate する one-shot。content 状態ではなく window runtime event。stale one-shot が resume 後に走ると flicker の原因になる。 |
| `fs_viewport_virtual_desktop_synced_hwnd` | `ViewerContextBundle` + `swap_field!` | active viewport runtime | HWND と一対の sync marker。host HWND を外すなら同時に外す。 |

### 2.2 Phase A2 で runtime 側へ移すか、少なくとも pause/resume で reset すべきもの

| field | 現在の位置 | 推奨 | 理由 / 注意 |
| --- | --- | --- | --- |
| `detached_viewer_focus_requested` | `ViewerContextBundle` + `swap_field!` | active viewport runtime の one-shot へ移す | resume 時に明示 focus したい意図は必要だが、paused content に残るべきではない。Phase A2 では `activate_detached_image_window_snapshot()` が runtime one-shot を立てる形にする。 |
| `detached_viewer_no_activate_once` | `ViewerContextBundle` + `swap_field!` | active viewport runtime の one-shot へ移す | viewport show 時の activation 制御。content bundle と一緒に pause しない。 |
| `still_fullscreen_viewport_enter_suppress_until` | `ViewerContextBundle` + `swap_field!` | active viewport runtime / App global | fullscreen viewport 入場時の transient。paused bundle から復元すると stale suppression になる。 |
| `fs_opened_at` | `ViewerContextBundle` + `swap_field!` | active viewport runtime / App global | focus grace 用時刻。content ではなく active viewport の入場時刻。 |
| `fs_focus_grace_elapsed` | `ViewerContextBundle` + `swap_field!` | active viewport runtime / App global | `fs_opened_at` と同じ。 |
| `fs_prev_focused` | `ViewerContextBundle` + `swap_field!` | active viewport runtime / App global | OS focus edge tracking。paused content へ保存すると、別 window の focus edge が混ざる。 |
| `fs_focus_regained_at` | `ViewerContextBundle` + `swap_field!` | active viewport runtime / App global | focus regain に伴う primary suppression の transient。 |
| `fs_prev_foreground_hwnd` | `ViewerContextBundle` + `swap_field!` | active viewport runtime / App global | native focus claim の前回 foreground。HWND transient。 |
| `fs_last_native_focus_claim_at` | `ViewerContextBundle` + `swap_field!` | active viewport runtime / App global | debouncing state。content ではない。 |
| `fs_last_main_focus_restore_at` | `ViewerContextBundle` + `swap_field!` | active viewport runtime / App global | focus restore debounce。content ではない。 |
| `fs_suppress_primary_until_release` | `ViewerContextBundle` + `swap_field!` | active viewport runtime / App global | pointer 入力 suppression。paused content へ戻すより、resume 時に明示 reset / set する。 |

### 2.3 per-window runtime へ寄せたいが、Phase A2 では慎重に扱うもの

| field | 現在の位置 | 推奨 | 理由 / 注意 |
| --- | --- | --- | --- |
| `detached_viewer_borderless_fullscreen` | `ViewerContextBundle` + `swap_field!` | per-window runtime | OS window の装飾 / 最大化相当の見た目状態。content ではない。ただし既存操作 F11 detached borderless の挙動に関わるため、Phase A2 ではまず active runtime に外し、passive window runtime への保持は別レビューでもよい。 |
| `detached_viewer_restore_placement` | `ViewerContextBundle` + `swap_field!` | per-window runtime | borderless 解除時の window placement。live placement と同じ系統なので runtime 所有が自然。 |
| `detached_viewer_borderless_transition` | `ViewerContextBundle` + `swap_field!` | active viewport runtime | DWM / window mode transition。paused bundle から復元しない。 |
| `pending_detached_video_host_switch` | `ViewerContextBundle` + `swap_field!` | 保留 | native video の host switch pending。今回の PDF / ZIP book detached とは別経路。動画 detached と bundle 分離の関係を別途確認してから動かす。 |

### 2.4 bundle に残すもの

| field | 理由 |
| --- | --- |
| `detached_viewer_window_id` | detached window identity。active と passive で同じ stable viewport id を使うため、book session / snapshot id と紐づける必要がある。ただし HWND ではない。 |
| `viewer_presentation` | content session が detached / fullscreen / main のどの presentation を要求しているか。実 OS viewport の shown state ではないため残す。 |
| `fullscreen_idx` | active book context の現在ページ。content state。 |
| `last_viewer_sync_stamp` | main grid 同期の意味状態。OS viewport transient ではない。 |
| `detached_viewer_independent_active` | linked / independent の session 意味状態。 |
| `detached_viewer_pin_active` | pin 操作の session 意味状態。 |
| `detached_viewer_open_next_still_detached_once` | 次回 open の routing 意図。OS HWND ではない。 |
| `fs_zoom`, `fs_pan`, `fs_vertical_scroll`, `spread_mode`, `reading_flow`, `reading_direction` | ユーザーの閲覧状態。pause / resume で保持すべき。 |
| `fs_cache`, `fs_pending`, `fs_upload_backlog`, `final_ai_cache`, `final_ai_pending` など | active book context の decode / AI / editing state。pause 時の cancel / backlog 方針は別途維持する。 |

補足: `fs_seek_drag_active` / `fs_seek_overlay_visible` / `fs_holdover_tex` /
`fs_nav_locked_gen` / 各 `*_drag_start` などの interaction transient は、viewport lifetime の
identity ではなく、pause 時の作業停止・入力状態 reset として扱う。既存の
`pause_background_work_keep_current_frame()` がこれらを clear / cancel するため、Phase A2 の
swap 除外対象にはしない。

## 3. 参照影響メモ

### 3.1 `render_fullscreen_viewport`

`render_fullscreen_viewport()` は active mount 中に次を直接参照する。

- `fs_viewport_shown`
- `fs_viewport_presentation`
- `detached_viewer_host_lost()`
- `detached_viewer_recreate_on_next_render`
- `fullscreen_viewport_id()`
- `detached_viewer_focus_requested`
- `detached_viewer_no_activate_once`
- `fs_prev_focused`
- `fs_prev_foreground_hwnd`
- `fs_last_native_focus_claim_at`
- `fs_focus_regained_at`
- `fs_suppress_primary_until_release`

Phase A2 では、これらが `ViewerContextBundle` 由来ではなく active viewport runtime 由来になる。
active detached context は常に 1 つなので、まずは App グローバル 1 組の runtime で足りる。

### 3.2 close / cleanup

以下は active viewport runtime を操作する。

- `keep_fullscreen_viewport_alive()`
- `hide_current_fullscreen_viewport_for_recreate()`
- `reset_detached_viewer_viewport_for_recreate()`
- `finalize_closed_active_detached_viewport()`
- `close_fullscreen()`
- `close_fullscreen_for_folder_nav_reopen()`

`fs_viewport_shown` を bundle から外すと、active context drop 後も cleanup state は App グローバルに残る。
これは今回の目的に合うが、main context 側で通常 fullscreen cleanup と detached active cleanup を混同しない
条件整理が必要。

### 3.3 pause / resume

`pause_current_active_detached_viewer_context()`:

- snapshot 作成前に active window の live HWND / live rect を取得する必要がある。
- `build_active_detached_image_window_snapshot()` は `detached_viewer_window_placement()` ではなく、
  live rect 由来 placement を受け取る形に変えるのが望ましい。
- paused bundle には viewport runtime を入れない。

`activate_detached_image_window_snapshot()`:

- paused bundle の古い HWND / shown / recreate flag を信用しない。
- snapshot id に対応する live passive viewport を active runtime へ transfer / recapture する。
- `detached_viewer_focus_requested` は bundle field ではなく runtime one-shot として立てる。

## 4. Phase A2 実装方針案

### 4.1 最小構造

まずは per-window map ではなく、active 用の App グローバル 1 組に寄せる。

```text
DetachedActiveViewportRuntime {
    shown: bool,
    presentation: Option<ViewerPresentation>,
    host_hwnd: u64,
    virtual_desktop_synced_hwnd: u64,
    generation: u64,
    recreate_after_hide: bool,
    recreate_on_next_render: bool,
    focus_requested: bool,
    no_activate_once: bool,
    opened_at: Option<Instant>,
    focus_grace_elapsed: bool,
    prev_focused: bool,
    focus_regained_at: Option<Instant>,
    prev_foreground_hwnd: usize,
    last_native_focus_claim_at: Option<Instant>,
    last_main_focus_restore_at: Option<Instant>,
    suppress_primary_until_release: bool,
}
```

既存 field をすぐ struct 化するか、Phase A2 では field 自体は App 直持ちのまま
`ViewerContextBundle` / `swap_field!` から外すだけでもよい。
ただし後続の per-window runtime 化を見据えるなら struct 化が望ましい。

### 4.2 A2 の実装順

1. `ViewerContextBundle` の destructure / `empty()` / `swap_field!` から 2.1 の field を外す。
2. コンパイルエラーで出る参照を active viewport runtime 参照へ寄せる。
3. `pause_current_active_detached_viewer_context()` で viewport runtime を paused bundle へ入れないことを確認する。
4. `activate_detached_image_window_snapshot()` で resume 時に stale runtime を復元しないことを確認する。
5. `MIV_DETACHED_WINDOW_DEBUG=1` で `host_lost_before_render` が出なくなるか確認する。

## 5. A1 レビュー依頼

ClaudeCode へ確認したい点:

1. 2.1 の「Phase A2 で bundle / swap から外すべきもの」に漏れや過剰除外はないか。
2. 2.2 の focus / input transient を同時に外すべきか、Phase A2 では reset 対応に留めるべきか。
3. borderless 系は Phase A2 に含めるべきか、Phase B 以降の per-window runtime 化へ分けるべきか。
4. `detached_viewer_window_id` を bundle に残す判断は正しいか。
5. active viewport runtime を App グローバル 1 組から始める方針で十分か。
