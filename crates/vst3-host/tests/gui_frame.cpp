#include "gui_frame.h"
#include "protocol.h"
#include "gui_visibility.h"
#include <cstdio>
#include <cstdlib>
#include <thread>
#include <tlhelp32.h>

namespace {
void check(bool value, const char* message) {
    if (!value) { std::fprintf(stderr, "gui frame failure: %s (Win32=%lu)\n", message, GetLastError()); std::exit(1); }
}
bool equal(const miv::NormalScreenRect& a, const miv::NormalScreenRect& b) {
    return a.x == b.x && a.y == b.y && a.outer_width == b.outer_width && a.outer_height == b.outer_height;
}
void desktop_diagnostic(const char* label, HDESK desktop) {
    wchar_t name[256]{}; DWORD needed = 0;
    GetUserObjectInformationW(desktop, UOI_NAME, name, sizeof(name), &needed);
    std::fprintf(stderr, "%s tid=%lu desktop=%p name=%ls\n", label, GetCurrentThreadId(), desktop, name);
}
bool desktop_failure_diagnostic(HDESK desktop) {
    HANDLE snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
    THREADENTRY32 entry{}; entry.dwSize = sizeof(entry);
    if (Thread32First(snapshot, &entry)) do {
        if (entry.th32OwnerProcessID == GetCurrentProcessId()) {
            std::fprintf(stderr, "remaining process thread=%lu\n", entry.th32ThreadID);
            desktop_diagnostic("thread desktop", GetThreadDesktop(entry.th32ThreadID));
        }
    } while (Thread32Next(snapshot, &entry));
    CloseHandle(snapshot);
    bool process_owned_helpers_only = true;
    const BOOL enumerated = EnumDesktopWindows(desktop, [](HWND hwnd, LPARAM context) -> BOOL {
        wchar_t klass[256]{}; GetClassNameW(hwnd, klass, 256);
        DWORD pid = 0; DWORD tid = GetWindowThreadProcessId(hwnd, &pid);
        std::fprintf(stderr, "remaining isolated window=%p tid=%lu pid=%lu class=%ls\n", hwnd, tid, pid, klass);
        if (pid != GetCurrentProcessId() || wcscmp(klass, L"MivInertEditorFrameTest") == 0)
            *reinterpret_cast<bool*>(context) = false;
        return TRUE;
    }, reinterpret_cast<LPARAM>(&process_owned_helpers_only));
    return enumerated && process_owned_helpers_only;
}
void pure_tests() {
    // Left/top taskbars and negative-coordinate secondary displays.
    RECT monitor{-1920, -1080, 0, 0};
    for (const RECT work : {RECT{-1880, -1080, 0, 0}, RECT{-1920, -1032, 0, 0},
                            RECT{-1920, -1080, -40, 0}, RECT{-1920, -1080, 0, -48}}) {
        const RECT placement{-1700, -900, -1000, -400};
        for (DWORD ex : {DWORD(WS_EX_APPWINDOW), DWORD(WS_EX_TOOLWINDOW)}) {
            auto screen = miv::placement_to_screen(placement, ex, monitor, work);
            check(screen.has_value(), "workspace conversion valid");
            check(screen->x == placement.left + ((ex & WS_EX_TOOLWINDOW) ? 0 : work.left - monitor.left), "screen x");
            check(screen->y == placement.top + ((ex & WS_EX_TOOLWINDOW) ? 0 : work.top - monitor.top), "screen y");
            auto roundtrip = miv::screen_to_placement(*screen, ex, monitor, work);
            check(roundtrip && EqualRect(&*roundtrip, &placement), "coordinate round trip");
        }
    }
    check(!miv::placement_to_screen(RECT{0, 0, 1, 10}, 0, RECT{}, RECT{}), "reject invalid width");
    check(!miv::placement_to_screen(RECT{INT32_MAX - 10, 0, INT32_MAX, 20}, 0, RECT{}, RECT{20, 0, 30, 30}), "reject overflow");
    check(!miv::screen_to_placement({INT32_MAX, 0, 20, 20}, WS_EX_TOOLWINDOW, RECT{}, RECT{}), "reject endpoint overflow");
    const DWORD generic = miv::editor_frame_style(false, true);
    check(generic == (WS_POPUP | WS_BORDER | WS_THICKFRAME | WS_CLIPCHILDREN | WS_CLIPSIBLINGS), "generic frame unchanged");
    const DWORD native = miv::editor_frame_style(true, false);
    check((native & (WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX)) == (WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX), "native caption contract");
    check(!(native & (WS_THICKFRAME | WS_MAXIMIZEBOX)), "fixed view no maximize/resize");
    check(!miv::editor_uses_saved_outer_size(true, false, true), "fixed native view ignores stale custom outer size");
    check(miv::editor_uses_saved_outer_size(true, true, true), "resizable native view retains saved size for constraint check");
    check(miv::editor_uses_saved_outer_size(false, false, true), "generic fixed view saved sizing unchanged");
    const DWORD other = WS_EX_WINDOWEDGE | WS_EX_TOPMOST | WS_EX_NOACTIVATE;
    check(miv::editor_taskbar_ex_style(other | WS_EX_TOOLWINDOW, true) == (other | WS_EX_APPWINDOW), "ON preserves unrelated exstyle");
    check(miv::editor_taskbar_ex_style(other | WS_EX_APPWINDOW, false) == (other | WS_EX_TOOLWINDOW), "OFF preserves unrelated exstyle");
    check(miv::normal_rect_json_member(miv::NormalScreenRect{-1700, -900, 700, 500}) ==
          ",\"normal_rect\":{\"x\":-1700,\"y\":-900,\"outer_width\":700,\"outer_height\":500}", "canonical wire");
    check(miv::normal_rect_json_member(std::nullopt).empty(), "failed geometry not substituted");
    check(miv::gui_warning_json(2, 99, "bad\n\"position\"") ==
          "{\"event\":\"gui_warning\",\"slot_id\":2,\"container_hwnd\":99,\"detail\":\"bad\\u000a\\\"position\\\"\"}", "warning wire escaped and frame scoped");
    for (WPARAM command : {WPARAM(SC_MINIMIZE), WPARAM(SC_RESTORE), WPARAM(SC_MAXIMIZE)})
        check(miv::editor_system_command(false, command, false) == miv::EditorSystemCommand::NativeDefault,
              "generic VST command policy unchanged");
    check(miv::editor_system_command(true, SC_MAXIMIZE, true) == miv::EditorSystemCommand::Block, "native maximize blocked");
    check(miv::editor_system_command(true, SC_RESTORE, false) == miv::EditorSystemCommand::Block, "suppressed restore blocked");
    for (UINT dpi : {96U, 144U, 192U}) {
        for (bool resizable : {false, true}) {
            for (bool taskbar : {false, true}) {
                const DWORD style = miv::editor_frame_style(true, resizable);
                const DWORD ex = miv::editor_taskbar_ex_style(WS_EX_WINDOWEDGE, taskbar);
                auto size = miv::editor_outer_size(style, ex, dpi, 600, 400);
                RECT nc{};
                check(size && AdjustWindowRectExForDpi(&nc, style, FALSE, ex, dpi), "DPI frame size");
                RECT client{0, 0, size->cx - (nc.right - nc.left), size->cy - (nc.bottom - nc.top)};
                const RECT child = miv::editor_child_bounds(client, dpi);
                check(child.right == 600 && child.bottom - child.top == 400, "plugin client preserved at DPI");
                check(child.top == miv::editor_status_height(dpi), "status band counted exactly once");
                check(child.top == (dpi == 96 ? 34 : dpi == 144 ? 51 : 68), "status band scales with DPI");
            }
        }
    }
    check(!miv::editor_outer_size(native, WS_EX_TOOLWINDOW, 96, 500, INT32_MAX), "size arithmetic overflow rejected");
}
struct FrameFixture {
    miv::GuiVisibility visibility;
    std::optional<miv::NormalScreenRect> before_minimize;
    std::optional<miv::NormalScreenRect> closed;
    int closes = 0;
    int shows = 0;
    int window_positions = 0;
};
LRESULT CALLBACK fixture_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    if (msg == WM_NCCREATE) {
        auto* create = reinterpret_cast<CREATESTRUCTW*>(lparam);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(create->lpCreateParams));
    }
    auto* fixture = reinterpret_cast<FrameFixture*>(GetWindowLongPtrW(hwnd, GWLP_USERDATA));
    if (fixture && msg == WM_SHOWWINDOW && wparam) ++fixture->shows;
    if (fixture && msg == WM_WINDOWPOSCHANGED) ++fixture->window_positions;
    if (fixture && msg == WM_SYSCOMMAND) {
        auto result = miv::handle_editor_system_command(true, wparam, fixture->visibility.should_show(true, true),
            [hwnd, fixture] {
                fixture->closed = miv::read_normal_screen_rect(hwnd);
                ++fixture->closes;
                fixture->visibility.request(false);
                ShowWindow(hwnd, SW_HIDE);
            },
            [hwnd, fixture] { fixture->before_minimize = miv::read_normal_screen_rect(hwnd); });
        if (result) return *result;
    }
    return DefWindowProcW(hwnd, msg, wparam, lparam);
}
void native_chrome_and_commands(WNDCLASSW& klass, const MONITORINFO& info) {
    for (bool resizable : {false, true}) {
        FrameFixture fixture;
        fixture.visibility.request(true);
        const DWORD style = miv::editor_frame_style(true, resizable), ex = WS_EX_APPWINDOW | WS_EX_WINDOWEDGE;
        UINT dpi = GetDpiForSystem(); if (!dpi) dpi = 96;
        const auto size = miv::editor_outer_size(style, ex, dpi, 500, 400);
        check(size.has_value(), "native initial size");
        HWND hwnd = CreateWindowExW(ex, klass.lpszClassName, L"EffeTune", style,
            info.rcWork.left + 100, info.rcWork.top + 100, size->cx, size->cy,
            nullptr, nullptr, klass.hInstance, &fixture);
        check(hwnd && miv::initialize_native_editor_frame(hwnd, dpi, resizable), "native icon/menu initialization");
        check(SendMessageW(hwnd, WM_GETICON, ICON_BIG, 0) && SendMessageW(hwnd, WM_GETICON, ICON_SMALL, 0), "large/small icon resources");
        HMENU menu = GetSystemMenu(hwnd, FALSE);
        check(menu != nullptr, "native system menu exists");
        for (UINT item : {UINT(SC_RESTORE), UINT(SC_MOVE), UINT(SC_MINIMIZE), UINT(SC_CLOSE)})
            check(GetMenuState(menu, item, MF_BYCOMMAND) != UINT(-1), "native system menu operation exists");
        check((GetMenuState(menu, SC_MAXIMIZE, MF_BYCOMMAND) & MF_GRAYED) != 0, "maximize menu disabled");
        check(((GetMenuState(menu, SC_SIZE, MF_BYCOMMAND) & MF_GRAYED) == 0) == resizable, "resize menu matches view");
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        RECT client{}; check(GetClientRect(hwnd, &client) != FALSE, "native client rect");
        auto child = miv::editor_child_bounds(client, GetDpiForWindow(hwnd));
        check(child.right == 500 && child.bottom - child.top == 400, "fixed/resizable initial view unclipped");
        POINT client_origin{0, 0}; check(ClientToScreen(hwnd, &client_origin) != FALSE, "client origin");
        const LPARAM status_point = MAKELPARAM(client_origin.x + 120, client_origin.y + 10);
        check(SendMessageW(hwnd, WM_NCHITTEST, 0, status_point) == HTCLIENT, "latency status band is not custom caption");
        const LPARAM caption_point = MAKELPARAM(client_origin.x + 120, client_origin.y - 10);
        check(SendMessageW(hwnd, WM_NCHITTEST, 0, caption_point) == HTCAPTION, "native caption hit testing");
        const auto initial = miv::read_normal_screen_rect(hwnd);
        check(initial.has_value(), "native initial normal snapshot");
        SendMessageW(hwnd, WM_SYSCOMMAND, SC_MAXIMIZE, 0);
        check(!IsZoomed(hwnd), "direct maximize rejected");
        SetWindowPos(hwnd, nullptr, initial->x + 25, initial->y + 20, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        auto moved = miv::read_normal_screen_rect(hwnd);
        SendMessageW(hwnd, WM_SYSCOMMAND, SC_MINIMIZE, 0);
        check(IsIconic(hwnd) && fixture.before_minimize && moved && equal(*fixture.before_minimize, *moved), "minimize captures latest normal geometry first");
        for (miv::GuiSuppression reason : {miv::GuiSuppression::RemoteSession, miv::GuiSuppression::Minimized}) {
            fixture.visibility.suppress(reason, true);
            ShowWindow(hwnd, SW_HIDE);
            SendMessageW(hwnd, WM_SYSCOMMAND, SC_RESTORE, 0);
            check(!IsWindowVisible(hwnd) && IsIconic(hwnd), "Remote/main suppression blocks shell restore");
            fixture.visibility.suppress(reason, false);
        }
        ShowWindow(hwnd, SW_SHOWMINNOACTIVE);
        SendMessageW(hwnd, WM_SYSCOMMAND, SC_RESTORE, 0);
        check(IsWindowVisible(hwnd) && !IsIconic(hwnd), "permitted shell restore uses OS native restore");
        SendMessageW(hwnd, WM_SYSCOMMAND, SC_MINIMIZE, 0);
        SendMessageW(hwnd, WM_SYSCOMMAND, SC_CLOSE, 0);
        check(fixture.closes == 1 && fixture.closed && moved && equal(*fixture.closed, *moved), "minimized close carries causal latest normal geometry");
        check(IsWindow(hwnd) && !IsWindowVisible(hwnd) && !fixture.visibility.requested(), "close hides retains HWND and request state");
        SendMessageW(hwnd, WM_SYSCOMMAND, SC_RESTORE, 0);
        check(!IsWindowVisible(hwnd), "user hidden blocks shell restore");
        check(DestroyWindow(hwnd) != FALSE, "native fixture window retired");
    }
}
void native_tests(HDESK desktop) {
    HDESK original_desktop = GetThreadDesktop(GetCurrentThreadId());
    desktop_diagnostic("worker original", original_desktop);
    check(SetThreadDesktop(desktop) != FALSE, "isolated desktop assignment");
    WNDCLASSW klass{};
    klass.hInstance = GetModuleHandleW(nullptr);
    klass.lpfnWndProc = fixture_proc;
    klass.lpszClassName = L"MivInertEditorFrameTest";
    check(RegisterClassW(&klass) != 0, "inert window class");
    MONITORINFO info{}; info.cbSize = sizeof(info);
    check(GetMonitorInfoW(MonitorFromPoint(POINT{0, 0}, MONITOR_DEFAULTTOPRIMARY), &info) != FALSE, "test work area");
    native_chrome_and_commands(klass, info);
    const int x = info.rcWork.left + 60, y = info.rcWork.top + 60;
    for (bool initially_visible : {false, true}) {
        for (bool iconic : {false, true}) {
            FrameFixture fixture;
            HWND hwnd = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_WINDOWEDGE, klass.lpszClassName, L"inert",
                miv::editor_frame_style(true, true), x, y, 640, 480, nullptr, nullptr, klass.hInstance, &fixture);
            check(hwnd != nullptr, "inert HWND");
            if (iconic) ShowWindow(hwnd, SW_SHOWMINNOACTIVE);
            if (!initially_visible) ShowWindow(hwnd, SW_HIDE);
            else if (!iconic) ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            const int no_op_positions = fixture.window_positions, no_op_shows = fixture.shows;
            bool no_op_permit_read = false;
            check(miv::update_editor_taskbar(hwnd, false, [&] { no_op_permit_read = true; return false; }), "same-style no-op succeeds");
            check(!no_op_permit_read && fixture.window_positions == no_op_positions && fixture.shows == no_op_shows,
                  "same-style no-op does not check permit or change window position/visibility");
            const auto original = miv::read_normal_screen_rect(hwnd);
            check(original.has_value(), "normal geometry acquired including iconic");
            const auto original_hwnd = hwnd;
            HWND foreground = GetForegroundWindow();
            for (bool taskbar : {true, false, true, false}) {
                check(miv::update_editor_taskbar(hwnd, taskbar, [] { return true; }), "same HWND taskbar update");
                check(IsWindow(hwnd) && hwnd == original_hwnd, "HWND retained");
                check(((GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_APPWINDOW) != 0) == taskbar, "taskbar target style");
                check(!!IsWindowVisible(hwnd) == initially_visible, "hidden/visible preserved");
                check(!!IsIconic(hwnd) == iconic, "iconic preserved");
                check(GetWindow(hwnd, GW_OWNER) == nullptr, "unowned preserved");
                check(GetForegroundWindow() == foreground, "inactive style change does not activate");
                auto rect = miv::read_normal_screen_rect(hwnd);
                check(rect && rect->x == original->x && rect->y == original->y, "screen top-left preserved");
                if (!taskbar) check(equal(*rect, *original), "client size roundtrip preserved");
            }
            check(miv::resize_normal_editor_frame(hwnd, 500, 400), "programmatic normal resize");
            check(!!IsWindowVisible(hwnd) == initially_visible && !!IsIconic(hwnd) == iconic, "resize preserves visibility/iconic");
            const auto close_rect = miv::read_normal_screen_rect(hwnd);
            check(close_rect && close_rect->x == original->x && close_rect->y == original->y, "close snapshot retains normal position");
            const auto expected = miv::editor_outer_size(DWORD(GetWindowLongPtrW(hwnd, GWL_STYLE)),
                DWORD(GetWindowLongPtrW(hwnd, GWL_EXSTYLE)), GetDpiForWindow(hwnd), 500, 400);
            check(expected && close_rect->outer_width == uint32_t(expected->cx) &&
                  close_rect->outer_height == uint32_t(expected->cy), "programmatic resize status/client/native sizing");
            const int prior_shows = fixture.shows;
            check(miv::update_editor_taskbar(hwnd, true, [] { return false; }), "suppressed style update");
            check(!IsWindowVisible(hwnd), "suppression never reveals frame");
            check(fixture.shows == prior_shows, "suppression does not briefly show then hide");
            check(DestroyWindow(hwnd) != FALSE, "style fixture window retired");
        }
    }
    check(UnregisterClassW(klass.lpszClassName, klass.hInstance) != FALSE, "inert class cleanup");
    check(SetThreadDesktop(original_desktop) != FALSE, "worker desktop release");
    desktop_diagnostic("worker released", GetThreadDesktop(GetCurrentThreadId()));
}
}
int main() {
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    pure_tests();
    // No app/plugin launch or interaction with the input desktop. This desktop
    // is never switched to and only this worker owns the inert test HWNDs.
    wchar_t name[80]{};
    swprintf_s(name, L"miv-gui-frame-test-%lu", GetCurrentProcessId());
    HDESK caller_desktop = GetThreadDesktop(GetCurrentThreadId());
    desktop_diagnostic("caller original", caller_desktop);
    HDESK desktop = CreateDesktopW(name, nullptr, nullptr, 0,
        DESKTOP_CREATEWINDOW | DESKTOP_CREATEMENU | DESKTOP_ENUMERATE | DESKTOP_READOBJECTS | DESKTOP_WRITEOBJECTS, nullptr);
    check(desktop != nullptr, "non-input test desktop");
    // CreateDesktop associates the calling thread too; release that borrowed
    // association before spawning the dedicated window owner.
    check(SetThreadDesktop(caller_desktop) != FALSE, "caller desktop release");
    desktop_diagnostic("caller released", GetThreadDesktop(GetCurrentThreadId()));
    std::thread worker([desktop] { native_tests(desktop); });
    worker.join();
    if (!CloseDesktop(desktop)) {
        const DWORD error = GetLastError();
        const bool helpers_only = desktop_failure_diagnostic(desktop);
        check(error == ERROR_BUSY && helpers_only, "isolated desktop cleanup ownership");
        // Native menu/caption input loads Windows TSF/Cicero helper threads.
        // All inert HWNDs and our class are already retired, and both owner
        // threads have released the desktop. Do not retry/wait on OS helpers;
        // this sole-owner test process exits now and reclaims its private object.
        std::fprintf(stderr, "private test desktop retained by process-owned native helpers (Win32=%lu); process exit reclaims it\n", error);
    }
    std::puts("gui frame geometry/style regressions passed");
}
