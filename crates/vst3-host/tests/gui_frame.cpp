#include "gui_frame.h"
#include "protocol.h"
#include "gui_visibility.h"
#include "host_app.h"
#include <cstdio>
#include <cstdlib>
#include <thread>
#include <memory>
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
    check(miv::editor_ex_style(other | WS_EX_TOOLWINDOW, true) == (other | WS_EX_APPWINDOW), "native frame preserves unrelated exstyle");
    check(miv::editor_ex_style(other | WS_EX_APPWINDOW, false) == (other | WS_EX_TOOLWINDOW), "generic TOOLWINDOW frame unchanged");
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
                const DWORD ex = miv::editor_ex_style(WS_EX_WINDOWEDGE, true);
                check(ex == (WS_EX_WINDOWEDGE | WS_EX_APPWINDOW), "ON/OFF use same native extended style");
                check(miv::editor_system_command(true, SC_MINIMIZE, true, taskbar) ==
                    (taskbar ? miv::EditorSystemCommand::PublishBeforeMinimize : miv::EditorSystemCommand::UserHide),
                    "taskbar policy controls minimize without changing the frame");
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
class FakeView : public Steinberg::IPlugView {
public:
    Steinberg::ViewRect size{0, 0, 600, 400};
    int notifications = 0;
    Steinberg::uint32 references = 1;
    Steinberg::tresult PLUGIN_API queryInterface(const Steinberg::TUID, void** out) override { *out = nullptr; return Steinberg::kNoInterface; }
    Steinberg::uint32 PLUGIN_API addRef() override { return ++references; }
    Steinberg::uint32 PLUGIN_API release() override { return --references; }
    Steinberg::tresult PLUGIN_API isPlatformTypeSupported(Steinberg::FIDString) override { return Steinberg::kResultTrue; }
    Steinberg::tresult PLUGIN_API attached(void*, Steinberg::FIDString) override { return Steinberg::kResultOk; }
    Steinberg::tresult PLUGIN_API removed() override { return Steinberg::kResultOk; }
    Steinberg::tresult PLUGIN_API onWheel(float) override { return Steinberg::kResultOk; }
    Steinberg::tresult PLUGIN_API onKeyDown(Steinberg::char16, Steinberg::int16, Steinberg::int16) override { return Steinberg::kResultOk; }
    Steinberg::tresult PLUGIN_API onKeyUp(Steinberg::char16, Steinberg::int16, Steinberg::int16) override { return Steinberg::kResultOk; }
    Steinberg::tresult PLUGIN_API getSize(Steinberg::ViewRect* out) override { *out = size; return Steinberg::kResultOk; }
    Steinberg::tresult PLUGIN_API onSize(Steinberg::ViewRect* requested) override { size = *requested; ++notifications; return Steinberg::kResultOk; }
    Steinberg::tresult PLUGIN_API onFocus(Steinberg::TBool) override { return Steinberg::kResultOk; }
    Steinberg::tresult PLUGIN_API setFrame(Steinberg::IPlugFrame*) override { return Steinberg::kResultOk; }
    Steinberg::tresult PLUGIN_API canResize() override { return Steinberg::kResultTrue; }
    Steinberg::tresult PLUGIN_API checkSizeConstraint(Steinberg::ViewRect*) override { return Steinberg::kResultOk; }
};
struct FrameFixture {
    miv::GuiVisibility visibility;
    std::optional<miv::NormalScreenRect> before_minimize;
    std::optional<miv::NormalScreenRect> closed;
    int closes = 0;
    int shows = 0;
    int window_positions = 0;
    int adds = 0, deletes = 0;
    HWND child = nullptr;
    FakeView* view = nullptr;
    uint32_t last_width = 0, last_height = 0;
    bool apply(HWND hwnd, bool force = true) {
        if (!view || !child) return false;
        return miv::apply_normal_editor_client(hwnd, child, last_width, last_height, force, [this](SIZE size) {
            Steinberg::ViewRect rect{0, 0, size.cx, size.cy};
            view->onSize(&rect);
        });
    }
    std::unique_ptr<miv::EditorTaskbar> taskbar;
    void sync(HWND hwnd, bool recreated = false) {
        if (!taskbar) return;
        if (recreated) check(SUCCEEDED(taskbar->shell_recreated()), "Shell recreation");
        check(SUCCEEDED(taskbar->sync(IsWindowVisible(hwnd) != FALSE, visibility.should_show(true, true))), "Shell membership sync");
    }
    void hide(HWND hwnd) {
        closed = miv::read_normal_screen_rect(hwnd);
        ++closes;
        visibility.request(false);
        ShowWindow(hwnd, SW_HIDE);
        sync(hwnd);
    }
    void set_taskbar(HWND hwnd, bool enabled) {
        taskbar->set_enabled(enabled);
        if (miv::editor_taskbar_requires_user_hide(enabled, IsIconic(hwnd) != FALSE)) hide(hwnd);
        else sync(hwnd);
    }
};
LRESULT CALLBACK fixture_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    if (msg == WM_NCCREATE) {
        auto* create = reinterpret_cast<CREATESTRUCTW*>(lparam);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(create->lpCreateParams));
    }
    auto* fixture = reinterpret_cast<FrameFixture*>(GetWindowLongPtrW(hwnd, GWLP_USERDATA));
    if (fixture && msg == WM_SHOWWINDOW && wparam) ++fixture->shows;
    if (fixture && msg == WM_WINDOWPOSCHANGED) ++fixture->window_positions;
    if (fixture && msg == WM_SIZE && wparam != SIZE_MINIMIZED) fixture->apply(hwnd, false);
    if (fixture && msg == WM_APP + 0x4D9) {
        check(miv::resize_normal_editor_frame(hwnd, static_cast<int>(wparam), static_cast<int>(lparam)), "plugin resize request");
        miv::layout_normal_editor_child(hwnd, fixture->child);
        return 0;
    }
    if (fixture && fixture->taskbar &&
        (msg == miv::editor_taskbar_button_created_message() || msg == miv::editor_taskbar_created_message())) {
        fixture->sync(hwnd, msg == miv::editor_taskbar_created_message());
        return 0;
    }
    if (fixture && msg == WM_SYSCOMMAND) {
        auto result = miv::handle_editor_system_command(true, wparam, fixture->visibility.should_show(true, true),
            [hwnd, fixture] { fixture->hide(hwnd); },
            [hwnd, fixture] { fixture->before_minimize = miv::read_normal_screen_rect(hwnd); },
            !fixture->taskbar || fixture->taskbar->enabled());
        if (result) return *result;
    }
    const LRESULT result = DefWindowProcW(hwnd, msg, wparam, lparam);
    if (fixture) {
        const auto* position = msg == WM_WINDOWPOSCHANGED ? reinterpret_cast<WINDOWPOS*>(lparam) : nullptr;
        if (msg == WM_SYSCOMMAND || (position && (position->flags & (SWP_SHOWWINDOW | SWP_HIDEWINDOW))))
            fixture->sync(hwnd);
    }
    return result;
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
void minimized_plugin_allocation_tests(WNDCLASSW& klass, const MONITORINFO& info) {
    for (bool native : {false, true}) for (bool resizable : {false, true}) for (bool taskbar : {false, true}) {
        FrameFixture fixture;
        FakeView view;
        fixture.view = &view;
        fixture.visibility.request(true);
        const DWORD style = miv::editor_frame_style(native, resizable);
        const DWORD ex = miv::editor_ex_style(WS_EX_WINDOWEDGE, native);
        const UINT dpi = GetDpiForSystem();
        const auto outer = miv::editor_outer_size(style, ex, dpi, 600, 400);
        check(outer.has_value(), "allocation initial geometry");
        HWND hwnd = CreateWindowExW(ex, klass.lpszClassName, L"allocation fixture", style,
            info.rcWork.left + 70, info.rcWork.top + 70, outer->cx, outer->cy,
            nullptr, nullptr, klass.hInstance, &fixture);
        check(hwnd != nullptr, "allocation inert parent");
        fixture.child = CreateWindowExW(0, L"STATIC", L"inert plugin child", WS_CHILD | WS_VISIBLE,
            0, 0, 1, 1, hwnd, nullptr, klass.hInstance, nullptr);
        check(fixture.child != nullptr && fixture.apply(hwnd), "normal hidden attach notifies plugin");
        check(view.size.right == 600 && view.size.bottom == 400 && fixture.last_width == 600 && fixture.last_height == 400,
              "confirmed normal allocation populates view and cached dimensions");
        fixture.taskbar = std::make_unique<miv::EditorTaskbar>(hwnd, taskbar, [](HWND, bool) { return S_OK; });
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        // Even OFF can receive an already-iconic frame (setting change or OS
        // transition). Allocation safety must not depend on taskbar policy.
        ShowWindow(hwnd, SW_SHOWMINNOACTIVE);
        check(IsIconic(hwnd), "allocation parent iconic");
        RECT child_before{}; GetClientRect(fixture.child, &child_before);
        const int notifications = view.notifications;
        const uint32_t last_width = fixture.last_width, last_height = fixture.last_height;
        miv::PlugFrame plug_frame;
        plug_frame.set_host_hwnd(hwnd);
        Steinberg::ViewRect requested{0, 0, 500, 300};
        check(plug_frame.resizeView(&view, &requested) == Steinberg::kResultOk, "actual PlugFrame accepts minimized plugin request");
        check(!fixture.apply(hwnd) && !miv::read_normal_editor_client(hwnd), "iconic refresh rejects client allocation");
        RECT child_after{}; GetClientRect(fixture.child, &child_after);
        check(EqualRect(&child_before, &child_after) && view.notifications == notifications &&
              fixture.last_width == last_width && fixture.last_height == last_height,
              "iconic resizeView/refresh preserve child view and last dimensions");
        // Host/DPI producers update only restore placement, using the same
        // production frame helper then normal allocation applicator.
        check(miv::resize_normal_editor_frame(hwnd, 550, 350) && !fixture.apply(hwnd), "host resize only changes iconic restore geometry");
        check(view.notifications == notifications && fixture.last_width == last_width && fixture.last_height == last_height,
              "iconic host resize leaves notification cache unchanged");
        const auto resized = miv::read_normal_screen_rect(hwnd);
        const auto expected_outer = miv::editor_outer_size(style, ex, GetDpiForWindow(hwnd), 550, 350);
        check(resized && expected_outer && resized->outer_width == uint32_t(expected_outer->cx) &&
              resized->outer_height == uint32_t(expected_outer->cy), "iconic requests update normal placement dimensions");
        fixture.visibility.suppress(miv::GuiSuppression::RemoteSession, true);
        ShowWindow(hwnd, SW_HIDE);
        fixture.visibility.suppress(miv::GuiSuppression::RemoteSession, false);
        ShowWindow(hwnd, SW_SHOWNA);
        check(IsIconic(hwnd) && !fixture.apply(hwnd) && view.notifications == notifications,
              "Remote unsuppress keeps iconic allocation and sends no onSize");
        SendMessageW(hwnd, WM_SYSCOMMAND, SC_CLOSE, 0);
        check(fixture.closed && resized && equal(*fixture.closed, *resized), "minimized Close snapshots latest normal placement");
        check(!fixture.apply(hwnd) && view.notifications == notifications, "hidden iconic Close/refresh never changes allocation");
        fixture.visibility.request(true);
        ShowWindow(hwnd, SW_SHOWNA);
        check(!fixture.apply(hwnd) && view.notifications == notifications, "manual Show pre-restore does not publish iconic client");
        ShowWindow(hwnd, SW_RESTORE);
        check(fixture.apply(hwnd) && !IsIconic(hwnd) && fixture.last_width == 550 && fixture.last_height == 350 &&
              view.size.right == 550 && view.size.bottom == 350, "manual restore publishes actual normal plugin client");
        // Normal suppressed resizeView acknowledgement is released behavior:
        // it still accepts the requested view size without resizing the shell.
        plug_frame.set_host_resize_enabled(false);
        const auto normal_before = miv::read_normal_screen_rect(hwnd);
        requested = {0, 0, 540, 340};
        const int normal_notifications = view.notifications;
        plug_frame.resizeView(&view, &requested);
        const auto normal_after = miv::read_normal_screen_rect(hwnd);
        check(normal_before && normal_after && equal(*normal_before, *normal_after) &&
              view.notifications == normal_notifications + 1 && view.size.right == 540 && view.size.bottom == 340,
              "normal suppressed resizeView requested acknowledgement preserved");
        fixture.taskbar.reset();
        check(DestroyWindow(hwnd) != FALSE, "allocation frame and inert child retired");
    }
}
LRESULT CALLBACK reentrant_child_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    if (msg == WM_SIZE) {
        HWND parent = reinterpret_cast<HWND>(GetWindowLongPtrW(hwnd, GWLP_USERDATA));
        if (parent) {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            ShowWindow(parent, SW_SHOWMINNOACTIVE);
        }
    }
    return DefWindowProcW(hwnd, msg, wparam, lparam);
}
void reentrant_allocation_test(WNDCLASSW& klass, const MONITORINFO& info) {
    WNDCLASSW child_class{};
    child_class.hInstance = klass.hInstance;
    child_class.lpfnWndProc = reentrant_child_proc;
    child_class.lpszClassName = L"MivReentrantInertPluginChild";
    check(RegisterClassW(&child_class) != 0, "reentrant child class");
    FrameFixture fixture;
    FakeView view;
    fixture.view = &view;
    fixture.visibility.request(true);
    HWND hwnd = CreateWindowExW(WS_EX_APPWINDOW, klass.lpszClassName, L"reentrant allocation",
        miv::editor_frame_style(true, true), info.rcWork.left + 80, info.rcWork.top + 80, 700, 500,
        nullptr, nullptr, klass.hInstance, &fixture);
    check(hwnd != nullptr, "reentrant parent");
    fixture.child = CreateWindowExW(0, child_class.lpszClassName, L"", WS_CHILD | WS_VISIBLE,
        0, 0, 1, 1, hwnd, nullptr, klass.hInstance, nullptr);
    check(fixture.child != nullptr, "reentrant child");
    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    // Force a changed child allocation so SetWindowPos enters child window code.
    SetWindowPos(fixture.child, nullptr, 0, 0, 1, 1, SWP_NOZORDER | SWP_NOACTIVATE);
    const int before_apply = view.notifications;
    fixture.last_width = 123; fixture.last_height = 456;
    SetWindowLongPtrW(fixture.child, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(hwnd));
    check(!fixture.apply(hwnd) && IsIconic(hwnd), "child layout reentrantly minimized parent");
    check(view.notifications == before_apply && fixture.last_width == 123 && fixture.last_height == 456,
          "reentrant minimization prevents stale onSize and cached dimension update");
    ShowWindow(hwnd, SW_RESTORE);
    check(fixture.apply(hwnd) && !IsIconic(hwnd), "later normal restore accepts client allocation");
    check(DestroyWindow(hwnd) != FALSE && UnregisterClassW(child_class.lpszClassName, child_class.hInstance),
          "reentrant fixture retired");
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
    minimized_plugin_allocation_tests(klass, info);
    reentrant_allocation_test(klass, info);
    const int x = info.rcWork.left + 60, y = info.rcWork.top + 60;
    for (bool initially_visible : {false, true}) {
        FrameFixture fixture;
        fixture.visibility.request(initially_visible);
        const DWORD ex = miv::editor_ex_style(WS_EX_WINDOWEDGE, true);
        HWND hwnd = CreateWindowExW(ex, klass.lpszClassName, L"EffeTune",
            miv::editor_frame_style(true, true), x, y, 640, 480, nullptr, nullptr, klass.hInstance, &fixture);
        check(hwnd != nullptr, "inert HWND");
        fixture.taskbar = std::make_unique<miv::EditorTaskbar>(hwnd, true, [&fixture, hwnd](HWND actual, bool add) {
            check(actual == hwnd, "Shell operations scoped to frame");
            if (add) ++fixture.adds; else ++fixture.deletes;
            return S_OK;
        });
        if (initially_visible) ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        const auto original = miv::read_normal_screen_rect(hwnd);
        RECT original_client{}; check(GetClientRect(hwnd, &original_client) != FALSE, "original native client");
        check(original.has_value(), "normal geometry acquired");
        const LONG_PTR style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        const int positions = fixture.window_positions, shows = fixture.shows;
        HWND foreground = GetForegroundWindow();
        for (bool taskbar : {false, true, false, true}) {
            const int adds = fixture.adds;
            fixture.set_taskbar(hwnd, taskbar);
            check(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) == ex && GetWindowLongPtrW(hwnd, GWL_STYLE) == style,
                  "ON/OFF leave native caption and styles identical");
            check(!!IsWindowVisible(hwnd) == initially_visible && !IsIconic(hwnd), "normal setting toggle preserves visibility");
            check(GetWindow(hwnd, GW_OWNER) == nullptr && GetForegroundWindow() == foreground, "owner/focus retained");
            const auto normal = miv::read_normal_screen_rect(hwnd);
            RECT client{}; GetClientRect(hwnd, &client);
            check(normal && equal(*normal, *original) && EqualRect(&client, &original_client), "normal and client geometry unchanged");
            check(fixture.window_positions == positions && fixture.shows == shows, "setting toggle neither shows nor reframes");
            if (!initially_visible) check(fixture.adds == adds, "hidden setting toggle never AddTab");
        }
        fixture.visibility.suppress(miv::GuiSuppression::RemoteSession, true);
        ShowWindow(hwnd, SW_HIDE);
        for (bool enabled : {false, true}) {
            fixture.set_taskbar(hwnd, enabled);
            const int adds = fixture.adds, shows_before = fixture.shows;
            SendMessageW(hwnd, miv::editor_taskbar_button_created_message(), 0, 0);
            SendMessageW(hwnd, miv::editor_taskbar_created_message(), 0, 0);
            check(fixture.adds == adds && fixture.shows == shows_before && !IsWindowVisible(hwnd),
                  "hidden/suppressed Shell events never add or show");
        }
        fixture.visibility.suppress(miv::GuiSuppression::RemoteSession, false);
        fixture.visibility.request(true);
        fixture.set_taskbar(hwnd, false);
        for (int repeat = 0; repeat != 3; ++repeat) {
            const int deletes = fixture.deletes;
            ShowWindow(hwnd, SW_SHOWNA); fixture.sync(hwnd);
            SendMessageW(hwnd, miv::editor_taskbar_button_created_message(), 0, 0);
            check(fixture.deletes >= deletes + 2 && IsWindowVisible(hwnd), "OFF reapplied after repeated show and Shell creation");
        }
        SendMessageW(hwnd, WM_SYSCOMMAND, SC_MINIMIZE, 0);
        check(!IsWindowVisible(hwnd) && !IsIconic(hwnd) && !fixture.visibility.requested(),
              "OFF minimize follows user hide without desktop iconic stub");
        check(fixture.closed && equal(*fixture.closed, *original), "OFF minimize publishes latest normal close geometry");
        SendMessageW(hwnd, WM_SYSCOMMAND, SC_RESTORE, 0);
        check(!IsWindowVisible(hwnd), "OFF user-hide rejects unsolicited restore");
        fixture.visibility.request(true);
        ShowWindow(hwnd, SW_SHOWNA); fixture.sync(hwnd);
        check(IsWindowVisible(hwnd) && !IsIconic(hwnd), "explicit button-equivalent Show reopens OFF user hide");
        fixture.set_taskbar(hwnd, true);
        SendMessageW(hwnd, WM_SYSCOMMAND, SC_MINIMIZE, 0);
        check(IsIconic(hwnd), "ON minimize retains native iconic behavior");
        check(miv::resize_normal_editor_frame(hwnd, 500, 400), "iconic programmatic resize");
        const auto resized = miv::read_normal_screen_rect(hwnd);
        check(resized && resized->x == original->x && resized->y == original->y, "iconic resize preserves normal origin");
        SendMessageW(hwnd, WM_SYSCOMMAND, SC_RESTORE, 0);
        check(!IsIconic(hwnd) && IsWindowVisible(hwnd), "ON shell restore native behavior");
        SendMessageW(hwnd, WM_SYSCOMMAND, SC_MINIMIZE, 0);
        fixture.set_taskbar(hwnd, false);
        check(!IsWindowVisible(hwnd) && !fixture.visibility.requested() && fixture.closed && equal(*fixture.closed, *resized),
              "iconic ON to OFF uses causal user hide with normal snapshot");
        fixture.set_taskbar(hwnd, true);
        const int hidden_adds = fixture.adds;
        SendMessageW(hwnd, miv::editor_taskbar_button_created_message(), 0, 0);
        check(fixture.adds == hidden_adds && !IsWindowVisible(hwnd), "hidden iconic OFF to ON does not reopen or add");
        fixture.visibility.request(true);
        ShowWindow(hwnd, SW_RESTORE); fixture.sync(hwnd);
        check(IsWindowVisible(hwnd) && !IsIconic(hwnd), "button-equivalent restore reopens hidden iconic frame");
        const int deletes = fixture.deletes;
        fixture.taskbar.reset();
        check(fixture.deletes == deletes + 1, "destructor removes explicitly added taskbar entry before HWND retirement");
        check(DestroyWindow(hwnd) != FALSE, "taskbar fixture window retired");
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
