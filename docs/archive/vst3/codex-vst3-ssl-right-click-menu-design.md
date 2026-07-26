# VST3 SSL Meter Pro Right-Click Menu Fix Design

## Context

SSL Meter Pro のプラグイン GUI で右クリックメニューが開いた直後に閉じる。
Rust 側の VST3 GUI host window はすでに `WM_PARENTNOTIFY` で
`SetForegroundWindow(host_hwnd)` を呼んでいるが、ユーザー検証では効果がない。

Insight2 のリサイズ中の中身遅延は、`WM_ENTERSIZEMOVE` / latest-only resize
coalescing で実用上改善済み。今回の対象は SSL Meter Pro の右クリックメニューのみ。

## Current Code

- Rust GUI host window:
  - `src/video/dsp/gui.rs`
  - `WM_PARENTNOTIFY` で child click を検出し、親 HWND を foreground にする。
- VST3 bridge:
  - `crates/vst3-host/src/plugin_loader.cpp`
  - `PluginLoader::show_gui()` が `IPlugView::attached(host_hwnd, HWND)` で
    プラグイン GUI を親 HWND に attach する。
  - `PluginLoader::hide_gui()` が `view_->removed()` で detach する。

## Likely Cause

SSL Meter Pro は右クリック時にプラグイン子 HWND 側で `TrackPopupMenu` 相当の処理を
走らせていると考えられる。トップレベルの host HWND ではなく、実際に右クリックを
受ける plugin child HWND のメッセージ処理より前に foreground / active window を
調整しないと、popup menu が owner / foreground 条件を満たせず即閉じる。

`WM_PARENTNOTIFY` は親 HWND に後追いで届くため、プラグイン側の popup menu 作成より
遅い、または必要な thread / active window 状態を満たせていない可能性が高い。

## Proposed Fix

Bridge 側で `IPlugView::attached()` 後に `EnumChildWindows(host_hwnd, ...)` を行い、
プラグインが作成した child HWND を `SetWindowSubclass` で subclass する。

Subclass callback では、右クリック関連メッセージをプラグイン本体に渡す直前に
host HWND を foreground / active にする。

対象メッセージ:

- `WM_RBUTTONDOWN`
- `WM_RBUTTONUP`
- `WM_CONTEXTMENU`
- `WM_NCRBUTTONDOWN`
- `WM_NCRBUTTONUP`

想定処理:

```cpp
static LRESULT CALLBACK PluginChildFocusSubclassProc(
    HWND hwnd,
    UINT msg,
    WPARAM wparam,
    LPARAM lparam,
    UINT_PTR subclass_id,
    DWORD_PTR ref_data)
{
    HWND host_hwnd = reinterpret_cast<HWND>(ref_data);
    if (is_right_click_or_context_menu(msg) && IsWindow(host_hwnd)) {
        SetForegroundWindow(host_hwnd);
        SetActiveWindow(host_hwnd);
    }
    if (msg == WM_NCDESTROY) {
        RemoveWindowSubclass(hwnd, PluginChildFocusSubclassProc, subclass_id);
    }
    return DefSubclassProc(hwnd, msg, wparam, lparam);
}
```

Use `SetWindowSubclass` rather than `SetWindowLongPtrW(GWLP_WNDPROC)` because it is
safer when the plugin or another library also subclasses the same child HWND.
This requires linking `comctl32`.

## Implementation Scope

### `crates/vst3-host/include/plugin_loader.h`

Add private helpers and tracked child HWNDs:

```cpp
void install_child_focus_hooks(void* host_hwnd);
void remove_child_focus_hooks();
std::vector<HWND> child_focus_hook_hwnds_;
```

`plugin_loader.h` currently does not include Windows types. Either include
`windows.h` in the header, or store `std::vector<void*>` and cast in the `.cpp`.
Prefer `std::vector<void*>` if we want to keep Windows headers out of the public
header.

### `crates/vst3-host/src/plugin_loader.cpp`

Add:

- `#include <CommCtrl.h>`
- Static subclass callback and enum callback.
- `PluginLoader::install_child_focus_hooks(void*)`
- `PluginLoader::remove_child_focus_hooks()`

Install hooks:

- After `view_->attached(...)` succeeds and `view_->onSize(...)` has been called.
- After `notify_host_resize(...)`, because some plugins lazily create or replace
  child HWNDs during resize.

Remove hooks:

- At the start of `hide_gui()`, before `view_->removed()`.
- In `PluginLoader::unload()` if GUI might still be attached.

Child eligibility:

- Only subclass child HWNDs owned by the bridge process:
  `GetWindowThreadProcessId(child, &pid)` and `pid == GetCurrentProcessId()`.
- Skip HWNDs already hooked by this subclass id.
- Keep hooks best-effort; hook failures should log and continue, not fail GUI open.

### `crates/vst3-host/CMakeLists.txt`

Link `comctl32` on Windows:

```cmake
target_link_libraries(mimageviewer-vst3-host PRIVATE
    ...
    comctl32
)
```

## Why Not WinEvent Hook First?

`SetWinEventHook(EVENT_OBJECT_CREATE, ...)` would catch child windows created after
attach without needing resize-triggered re-enumeration. However it adds a longer
lifetime model, callback filtering, and teardown complexity.

Start with deterministic re-enumeration:

1. after attach
2. after `onSize`
3. after each host resize notification

If SSL Meter Pro still creates the popup owner HWND too late, add a narrow
WinEvent hook as Phase 2.

## Risks

- Some plugins may create child HWNDs after attach and before the first resize.
  Mitigation: enumerate after `attached()` and after `onSize()`; add WinEvent hook
  only if needed.
- Subclass callback must not block or call into VST3 APIs. It should only adjust
  foreground/active state and forward to `DefSubclassProc`.
- `SetForegroundWindow` can be restricted by Windows foreground rules, but the
  call is triggered by direct user input in the same interaction path, so it has
  the best chance of succeeding.
- If a plugin child HWND is destroyed, `WM_NCDESTROY` removes the subclass.
  `hide_gui()` also removes tracked hooks as a second safety net.

## Validation Plan

1. Build bridge:
   `cmake --build crates/vst3-host/build --config Release`
2. Confirm `vendor/vst3-host/mimageviewer-vst3-host.exe` timestamp updates.
3. Run mIV with VST3 enabled and SSL Meter Pro GUI open.
4. Right-click the SSL Meter Pro GUI:
   - menu remains open
   - plugin resize/menu command can be selected
   - normal left-click plugin operation still works
5. Regression check:
   - Insight2 resize still tracks without major lag
   - Pro-Q / existing plugin GUI open-close still works
   - closing plugin GUI does not crash bridge

## Review Questions For ClaudeCode

1. Is `SetWindowSubclass` + `comctl32` acceptable for the bridge, or should we
   avoid the new dependency and use `SetWindowLongPtrW`?
2. Is re-enumerating child HWNDs after attach/onSize/resize enough for SSL Meter
   Pro, or should Phase 1 include a WinEvent create hook?
3. Should the subclass adjust only `SetForegroundWindow(host_hwnd)`, or also call
   `SetActiveWindow(host_hwnd)` / `SetFocus(hwnd)`?
4. Should Rust-side `WM_PARENTNOTIFY` foreground handling remain as a fallback,
   or be removed after bridge-side hooks are verified?

## ClaudeCode Review Result

- Use `SetWindowSubclass` + `comctl32`; avoid raw `SetWindowLongPtrW`.
- Phase 1 should use deterministic child enumeration after attach/onSize/resize.
  Do not add a WinEvent hook yet.
- Start with `SetForegroundWindow(host_hwnd)` only. User testing showed SSL
  Meter Pro still closes its graphical menu, while Pro-Q's simple text menu
  works. Phase 1.1 therefore adds `SetFocus(child_hwnd)` before forwarding the
  right-click message.
- Keep Rust-side `WM_PARENTNOTIFY` foreground handling as a fallback until
  multiple plugin GUIs have been verified.
- Keep subclass callbacks minimal; do not mutate `PluginLoader` state from
  `WM_NCDESTROY`. Sweep stale HWNDs from the tracked vector during later
  enumeration/removal.

## Implementation Notes

Implemented Phase 1 with:

- `PluginLoader::install_child_focus_hooks(void*)`
- `PluginLoader::remove_child_focus_hooks()`
- `SetWindowSubclass` callback on bridge-owned plugin child HWNDs
- Re-enumeration after GUI attach/onSize and after host resize notification
- `comctl32` link dependency in the VST3 bridge target
- Phase 1.1 focus nudge: `SetForegroundWindow(host_hwnd)` +
  `SetFocus(child_hwnd)` for SSL Meter Pro's graphical context menu.
- Phase 1.2 conflict isolation: Rust-side `WM_PARENTNOTIFY` keeps the left-click
  foreground fallback, but no longer handles right-click. This avoids a
  follow-up parent notification undoing the bridge-side child focus immediately
  before SSL Meter Pro opens its graphical menu. The bridge now logs
  `right-click subclass` lines so we can confirm whether the child subclass path
  actually runs during SSL Meter Pro right-clicks.
- Phase 1.3 diagnostic result: user testing still closed the SSL Meter Pro menu,
  and logs showed child HWND hooks were installed but `right-click subclass`
  never fired. This means SSL Meter Pro's right-click path does not pass through
  the enumerated child HWND WndProc. Add a thread-local `WH_MOUSE` hook for the
  bridge-owned plugin GUI thread(s), so right-click messages on later-created or
  non-enumerated HWNDs can still run the same foreground/focus fix before the
  plugin handles them.
- Phase 1.4 diagnostic result: after fixing stale bridge extraction, logs show
  the SSL Meter Pro child HWND does receive `WM_RBUTTONDOWN`, then captures the
  mouse and receives `WM_RBUTTONUP` followed immediately by
  `WM_CAPTURECHANGED`. Swallowing those messages made repeated right-clicks
  worse, so that experiment was removed.
- Phase 1.5 bridge-owned top-level surface: Bitwig Studio works with SSL Meter
  Pro even when VSTs are process-isolated, while mIV attached the plugin view
  directly to a Rust-process HWND. mIV now asks the bridge to create a
  `WS_POPUP` tool window owned by the Rust host HWND, and passes that bridge
  HWND to `IPlugView::attached()`. The plugin's top-level ancestor now lives in
  the bridge process, so SSL Meter Pro's popup/menu owner chain no longer has to
  run through a cross-process child parent. The Rust host HWND remains the outer
  placement/resizing shell; resize and move notifications reposition the bridge
  surface over the host client area. The bridge installs the foreground/focus
  hook on both the bridge surface itself and its child HWNDs, because some VST3
  editors handle mouse messages directly on the attached HWND instead of
  creating a separate child window.
- Phase 1.6 top-level surface hardening: the bridge surface is created hidden
  and shown only after `attached()` + `onSize()` complete, preventing a blank
  top-level rectangle during slow plugin attach. `notify_host_resize` always
  repositions the bridge surface, but skips `view->onSize()` and child HWND
  re-enumeration when the size did not change, so host-window moves do not drive
  expensive plugin relayout. Rust now sends `set_gui_visible` to the bridge when
  an existing GUI is hidden/shown, because `ShowWindow(SW_HIDE)` on the Rust
  owner does not reliably hide a cross-process owned popup.
