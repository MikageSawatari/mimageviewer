#include "gui_frame.h"
#include <limits>

namespace miv {
namespace {
bool valid_long(int64_t n) { return n >= INT32_MIN && n <= INT32_MAX; }
std::optional<NormalScreenRect> shifted_rect(const RECT& r, int64_t dx, int64_t dy) {
    const int64_t x = int64_t(r.left) + dx, y = int64_t(r.top) + dy;
    const int64_t right = int64_t(r.right) + dx, bottom = int64_t(r.bottom) + dy;
    const int64_t w = right - x, h = bottom - y;
    if (!valid_long(x) || !valid_long(y) || !valid_long(right) || !valid_long(bottom) ||
        w <= 1 || h <= 1 || w > INT32_MAX || h > INT32_MAX) return std::nullopt;
    return NormalScreenRect{int32_t(x), int32_t(y), uint32_t(w), uint32_t(h)};
}
bool monitor_info(HWND hwnd, MONITORINFO& info) {
    info.cbSize = sizeof(info);
    HMONITOR monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
    return monitor && GetMonitorInfoW(monitor, &info);
}
bool frame_insets(HWND hwnd, DWORD ex_style, RECT& insets) {
    UINT dpi = GetDpiForWindow(hwnd);
    if (!dpi) dpi = 96;
    insets = RECT{};
    const DWORD style = DWORD(GetWindowLongPtrW(hwnd, GWL_STYLE)) & ~(WS_MINIMIZE | WS_MAXIMIZE);
    return AdjustWindowRectExForDpi(&insets, style, FALSE, ex_style, dpi) != FALSE;
}
bool set_hidden_normal_rect(HWND hwnd, const NormalScreenRect& rect, DWORD ex_style) {
    MONITORINFO info{};
    WINDOWPLACEMENT placement{};
    placement.length = sizeof(placement);
    if (!monitor_info(hwnd, info) || !GetWindowPlacement(hwnd, &placement)) return false;
    auto converted = screen_to_placement(rect, ex_style, info.rcMonitor, info.rcWork);
    if (!converted) return false;
    placement.rcNormalPosition = *converted;
    placement.flags &= ~WPF_RESTORETOMAXIMIZED;
    placement.showCmd = SW_HIDE; // placement must not reveal suppressed/hidden windows
    return SetWindowPlacement(hwnd, &placement) != FALSE;
}
}
std::optional<NormalScreenRect> placement_to_screen(const RECT& normal, DWORD ex_style,
                                                   const RECT& monitor, const RECT& work) {
    const bool workspace = (ex_style & WS_EX_TOOLWINDOW) == 0;
    return shifted_rect(normal, workspace ? int64_t(work.left) - monitor.left : 0,
                        workspace ? int64_t(work.top) - monitor.top : 0);
}
std::optional<RECT> screen_to_placement(const NormalScreenRect& normal, DWORD ex_style,
                                       const RECT& monitor, const RECT& work) {
    const bool workspace = (ex_style & WS_EX_TOOLWINDOW) == 0;
    const int64_t x = int64_t(normal.x) - (workspace ? int64_t(work.left) - monitor.left : 0);
    const int64_t y = int64_t(normal.y) - (workspace ? int64_t(work.top) - monitor.top : 0);
    const int64_t r = x + normal.outer_width, b = y + normal.outer_height;
    if (normal.outer_width <= 1 || normal.outer_height <= 1 || normal.outer_width > INT32_MAX ||
        normal.outer_height > INT32_MAX || !valid_long(x) || !valid_long(y) ||
        !valid_long(r) || !valid_long(b)) return std::nullopt;
    return RECT{LONG(x), LONG(y), LONG(r), LONG(b)};
}
DWORD editor_frame_style(bool native_caption, bool resizable) {
    return WS_POPUP | (native_caption ? WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX : WS_BORDER) |
           (resizable ? WS_THICKFRAME : 0) | WS_CLIPCHILDREN | WS_CLIPSIBLINGS;
}
bool editor_uses_saved_outer_size(bool native_caption, bool resizable, bool has_saved_size) {
    // A fixed EffeTune view must use its required client size rather than a
    // saved outer size from the older custom-caption frame.
    return has_saved_size && (!native_caption || resizable);
}
DWORD editor_taskbar_ex_style(DWORD current, bool show_in_taskbar) {
    return (current & ~(WS_EX_APPWINDOW | WS_EX_TOOLWINDOW)) |
           (show_in_taskbar ? WS_EX_APPWINDOW : WS_EX_TOOLWINDOW);
}
EditorSystemCommand editor_system_command(bool native_frame, WPARAM command, bool restore_permitted) {
    const auto action = command & 0xFFF0;
    if (action == SC_CLOSE) return EditorSystemCommand::UserHide;
    if (!native_frame) return EditorSystemCommand::NativeDefault;
    if (action == SC_MAXIMIZE || (action == SC_RESTORE && !restore_permitted)) return EditorSystemCommand::Block;
    if (action == SC_MINIMIZE) return EditorSystemCommand::PublishBeforeMinimize;
    return EditorSystemCommand::NativeDefault;
}
std::optional<LRESULT> handle_editor_system_command(bool native_frame, WPARAM command,
    bool restore_permitted, const std::function<void()>& user_hide,
    const std::function<void()>& before_minimize) {
    switch (editor_system_command(native_frame, command, restore_permitted)) {
        case EditorSystemCommand::UserHide: user_hide(); return 0;
        case EditorSystemCommand::Block: return 0;
        case EditorSystemCommand::PublishBeforeMinimize: before_minimize(); break;
        case EditorSystemCommand::NativeDefault: break;
    }
    return std::nullopt;
}
int editor_status_height(UINT dpi) {
    const int scaled = MulDiv(34, static_cast<int>(dpi ? dpi : 96), 96);
    return scaled > 28 ? scaled : 28;
}
RECT editor_child_bounds(const RECT& client, UINT dpi) {
    const int status = editor_status_height(dpi);
    return RECT{0, status, client.right > 0 ? client.right : 1,
                client.bottom > status ? client.bottom : status + 1};
}
std::optional<SIZE> editor_outer_size(DWORD style, DWORD ex_style, UINT dpi,
                                     int plugin_width, int plugin_height) {
    const int status = editor_status_height(dpi);
    if (plugin_width <= 0 || plugin_height <= 0 || plugin_height > INT32_MAX - status) return std::nullopt;
    RECT outer{0, 0, plugin_width, plugin_height + status};
    if (!AdjustWindowRectExForDpi(&outer, style & ~(WS_MINIMIZE | WS_MAXIMIZE), FALSE, ex_style, dpi ? dpi : 96))
        return std::nullopt;
    const int64_t width = int64_t(outer.right) - outer.left, height = int64_t(outer.bottom) - outer.top;
    if (width <= 1 || height <= 1 || width > INT32_MAX || height > INT32_MAX) return std::nullopt;
    return SIZE{LONG(width), LONG(height)};
}
bool initialize_native_editor_frame(HWND hwnd, UINT dpi, bool resizable) {
    HINSTANCE instance = GetModuleHandleW(nullptr);
    HICON big = static_cast<HICON>(LoadImageW(instance, MAKEINTRESOURCEW(101), IMAGE_ICON,
        GetSystemMetricsForDpi(SM_CXICON, dpi), GetSystemMetricsForDpi(SM_CYICON, dpi), LR_SHARED));
    HICON small = static_cast<HICON>(LoadImageW(instance, MAKEINTRESOURCEW(101), IMAGE_ICON,
        GetSystemMetricsForDpi(SM_CXSMICON, dpi), GetSystemMetricsForDpi(SM_CYSMICON, dpi), LR_SHARED));
    if (!big || !small) return false;
    SendMessageW(hwnd, WM_SETICON, ICON_BIG, reinterpret_cast<LPARAM>(big));
    SendMessageW(hwnd, WM_SETICON, ICON_SMALL, reinterpret_cast<LPARAM>(small));
    HMENU menu = GetSystemMenu(hwnd, FALSE);
    if (!menu) return false;
    EnableMenuItem(menu, SC_MAXIMIZE, MF_BYCOMMAND | MF_GRAYED);
    EnableMenuItem(menu, SC_SIZE, MF_BYCOMMAND | (resizable ? MF_ENABLED : MF_GRAYED));
    return true;
}
std::optional<NormalScreenRect> read_normal_screen_rect(HWND hwnd) {
    WINDOWPLACEMENT placement{};
    placement.length = sizeof(placement);
    MONITORINFO info{};
    if (!IsWindow(hwnd) || !GetWindowPlacement(hwnd, &placement) || !monitor_info(hwnd, info))
        return std::nullopt;
    auto rect = placement_to_screen(placement.rcNormalPosition,
        DWORD(GetWindowLongPtrW(hwnd, GWL_EXSTYLE)), info.rcMonitor, info.rcWork);
    if (!rect) return std::nullopt;
    RECT screen{rect->x, rect->y, LONG(int64_t(rect->x) + rect->outer_width),
                LONG(int64_t(rect->y) + rect->outer_height)}, intersection{};
    HMONITOR monitor = MonitorFromRect(&screen, MONITOR_DEFAULTTONEAREST);
    info.cbSize = sizeof(info);
    if (!monitor || !GetMonitorInfoW(monitor, &info) ||
        !IntersectRect(&intersection, &screen, &info.rcWork)) return std::nullopt;
    return rect;
}
bool update_editor_taskbar(HWND hwnd, bool show_in_taskbar, const std::function<bool()>& permit) {
    const DWORD old_ex = DWORD(GetWindowLongPtrW(hwnd, GWL_EXSTYLE));
    const DWORD new_ex = editor_taskbar_ex_style(old_ex, show_in_taskbar);
    if (new_ex == old_ex) return true;
    auto normal = read_normal_screen_rect(hwnd);
    RECT old_insets{}, new_insets{};
    if (!normal || !frame_insets(hwnd, old_ex, old_insets) || !frame_insets(hwnd, new_ex, new_insets)) return false;
    const int64_t width = int64_t(normal->outer_width) - (old_insets.right - old_insets.left) +
                         (new_insets.right - new_insets.left);
    const int64_t height = int64_t(normal->outer_height) - (old_insets.bottom - old_insets.top) +
                          (new_insets.bottom - new_insets.top);
    if (width <= 1 || height <= 1 || width > INT32_MAX || height > INT32_MAX) return false;
    normal->outer_width = uint32_t(width);
    normal->outer_height = uint32_t(height);
    const bool visible = IsWindowVisible(hwnd) != FALSE, iconic = IsIconic(hwnd) != FALSE;
    const bool foreground = GetAncestor(GetForegroundWindow(), GA_ROOT) == hwnd;
    if (visible) ShowWindow(hwnd, SW_HIDE);
    SetLastError(0);
    if (!SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_ex) && GetLastError()) return false;
    if (!SetWindowPos(hwnd, nullptr, 0, 0, 0, 0,
                      SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) ||
        !set_hidden_normal_rect(hwnd, *normal, new_ex)) return false;
    if (visible && permit()) {
        ShowWindow(hwnd, iconic ? SW_SHOWMINNOACTIVE : SW_SHOWNOACTIVATE);
        if (!permit()) ShowWindow(hwnd, SW_HIDE);
        else if (foreground && !iconic) SetForegroundWindow(hwnd);
    }
    return true;
}
bool resize_normal_editor_frame(HWND hwnd, int plugin_width, int plugin_height) {
    if (plugin_width <= 0 || plugin_height <= 0) return false;
    const DWORD ex = DWORD(GetWindowLongPtrW(hwnd, GWL_EXSTYLE));
    UINT dpi = GetDpiForWindow(hwnd); if (!dpi) dpi = 96;
    const auto outer = editor_outer_size(DWORD(GetWindowLongPtrW(hwnd, GWL_STYLE)), ex, dpi,
                                        plugin_width, plugin_height);
    if (!outer) return false;
    if (IsIconic(hwnd)) {
        auto normal = read_normal_screen_rect(hwnd);
        if (!normal) return false;
        normal->outer_width = uint32_t(outer->cx);
        normal->outer_height = uint32_t(outer->cy);
        // A minimized programmatic resize changes restore geometry only.
        WINDOWPLACEMENT p{}; p.length = sizeof(p); MONITORINFO info{};
        if (!GetWindowPlacement(hwnd, &p) || !monitor_info(hwnd, info)) return false;
        auto converted = screen_to_placement(*normal, ex, info.rcMonitor, info.rcWork);
        if (!converted) return false;
        p.rcNormalPosition = *converted;
        p.showCmd = IsWindowVisible(hwnd) ? SW_SHOWMINNOACTIVE : SW_HIDE;
        return SetWindowPlacement(hwnd, &p) != FALSE;
    }
    return SetWindowPos(hwnd, nullptr, 0, 0, outer->cx, outer->cy,
        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS) != FALSE;
}
}
