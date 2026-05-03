// VST3 プラグインのロード & process loop 実装
//
// Phase 0 POC では「ロードしてパススルーで音を通す」のが目的。
// IComponent / IAudioProcessor / IEditController の取得と最低限の lifecycle 制御まで実装する。

#include "plugin_loader.h"

#include <algorithm>
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <deque>
#include <cstdio>
#include <cstdint>
#include <functional>
#include <future>
#include <mutex>
#include <optional>
#include <thread>
#include <type_traits>
#include <vector>

#include <windows.h>
#include <windowsx.h>
#include <dwmapi.h>
#include <ole2.h>
#include "pluginterfaces/gui/iplugviewcontentscalesupport.h"

namespace miv {
void send_event_gui_user_hidden(uint64_t slot_id);
}

namespace {
// stderr へのデバッグログ。tester 側で pipe して log_file に流す。
template <typename... Args>
void blog(const char* fmt, Args... args) {
    std::fprintf(stderr, "[BRIDGE] ");
    std::fprintf(stderr, fmt, args...);
    std::fprintf(stderr, "\n");
    std::fflush(stderr);
}
inline void blog(const char* msg) {
    std::fprintf(stderr, "[BRIDGE] %s\n", msg);
    std::fflush(stderr);
}

constexpr const wchar_t* kBridgeViewContainerClass = L"MivVst3BridgeViewContainer";
constexpr const wchar_t* kBridgePluginHostClass = L"MivVst3PluginHost";
constexpr UINT kBridgeResizePluginClientMsg = WM_APP + 0x4D9;

#ifndef DWMWA_USE_IMMERSIVE_DARK_MODE
#define DWMWA_USE_IMMERSIVE_DARK_MODE 20
#endif
#ifndef DWMWA_CAPTION_COLOR
#define DWMWA_CAPTION_COLOR 35
#endif
#ifndef DWMWA_TEXT_COLOR
#define DWMWA_TEXT_COLOR 36
#endif
#ifndef DWMWA_BORDER_COLOR
#define DWMWA_BORDER_COLOR 34
#endif

std::wstring utf8_to_wide(const std::string& text) {
    if (text.empty()) {
        return L"VST3 Plugin";
    }
    int needed = MultiByteToWideChar(CP_UTF8, 0, text.c_str(), -1, nullptr, 0);
    if (needed <= 1) {
        return L"VST3 Plugin";
    }
    std::wstring out(static_cast<size_t>(needed - 1), L'\0');
    MultiByteToWideChar(CP_UTF8, 0, text.c_str(), -1, out.data(), needed);
    return out;
}

int editor_titlebar_height(HWND hwnd) {
    UINT dpi = hwnd ? GetDpiForWindow(hwnd) : GetDpiForSystem();
    if (dpi == 0) dpi = 96;
    return std::max(28, MulDiv(34, static_cast<int>(dpi), 96));
}

RECT editor_close_button_rect(HWND hwnd) {
    RECT client{};
    GetClientRect(hwnd, &client);
    const int title_h = editor_titlebar_height(hwnd);
    const int button = std::max(18, title_h - 10);
    const int top = std::max(0, (title_h - button) / 2);
    return RECT{client.right - button - 8, top, client.right - 8, top + button};
}

bool point_in_rect(const RECT& rect, POINT pt) {
    return pt.x >= rect.left && pt.x < rect.right && pt.y >= rect.top && pt.y < rect.bottom;
}

void layout_editor_child(HWND frame_hwnd, HWND child_hwnd) {
    if (!frame_hwnd || !child_hwnd || !IsWindow(frame_hwnd) || !IsWindow(child_hwnd)) {
        return;
    }
    RECT client{};
    if (!GetClientRect(frame_hwnd, &client)) {
        return;
    }
    const int title_h = editor_titlebar_height(frame_hwnd);
    const int width = std::max<LONG>(1, client.right - client.left);
    const int height = std::max<LONG>(1, client.bottom - client.top - title_h);
    SetWindowPos(child_hwnd,
                 nullptr,
                 0,
                 title_h,
                 width,
                 height,
                 SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS);
    RedrawWindow(child_hwnd,
                 nullptr,
                 nullptr,
                 RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW);
}

bool resize_frame_for_plugin_client(HWND frame_hwnd, int plugin_w, int plugin_h) {
    if (!frame_hwnd || !IsWindow(frame_hwnd) || plugin_w <= 0 || plugin_h <= 0) {
        return false;
    }
    const int title_h = editor_titlebar_height(frame_hwnd);
    RECT outer{0, 0, plugin_w, plugin_h + title_h};
    DWORD style = static_cast<DWORD>(GetWindowLongPtrW(frame_hwnd, GWL_STYLE));
    DWORD ex_style = static_cast<DWORD>(GetWindowLongPtrW(frame_hwnd, GWL_EXSTYLE));
    UINT dpi = GetDpiForWindow(frame_hwnd);
    if (dpi == 0) dpi = 96;
    AdjustWindowRectExForDpi(&outer, style, FALSE, ex_style, dpi);
    SetWindowPos(frame_hwnd,
                 nullptr,
                 0,
                 0,
                 std::max<LONG>(1, outer.right - outer.left),
                 std::max<LONG>(1, outer.bottom - outer.top),
                 SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS);
    return true;
}

void hide_editor_surface_from_user(HWND hwnd, miv::PluginLoader* loader) {
    if (loader) {
        loader->set_gui_surface_visible_state(false);
        miv::send_event_gui_user_hidden(loader->editor_chrome_slot_id());
    }
    ShowWindow(hwnd, SW_HIDE);
}

void draw_editor_chrome(HWND hwnd, miv::PluginLoader* loader) {
    PAINTSTRUCT ps{};
    HDC hdc = BeginPaint(hwnd, &ps);
    if (!hdc) return;
    RECT client{};
    GetClientRect(hwnd, &client);
    RECT title_rect{0, 0, client.right, editor_titlebar_height(hwnd)};
    HBRUSH bg = CreateSolidBrush(RGB(18, 18, 18));
    FillRect(hdc, &client, bg);
    DeleteObject(bg);

    RECT close_rect = editor_close_button_rect(hwnd);
    HBRUSH close_bg = CreateSolidBrush(RGB(38, 38, 38));
    FillRect(hdc, &close_rect, close_bg);
    DeleteObject(close_bg);
    HPEN close_pen = CreatePen(PS_SOLID, 2, RGB(230, 230, 230));
    HGDIOBJ old_pen = SelectObject(hdc, close_pen);
    MoveToEx(hdc, close_rect.left + 6, close_rect.top + 6, nullptr);
    LineTo(hdc, close_rect.right - 6, close_rect.bottom - 6);
    MoveToEx(hdc, close_rect.right - 6, close_rect.top + 6, nullptr);
    LineTo(hdc, close_rect.left + 6, close_rect.bottom - 6);
    SelectObject(hdc, old_pen);
    DeleteObject(close_pen);

    SetBkMode(hdc, TRANSPARENT);
    SetTextColor(hdc, RGB(228, 228, 228));
    std::wstring title = utf8_to_wide(loader ? loader->editor_chrome_title() : std::string{});
    if (loader) {
        const uint32_t latency = loader->editor_chrome_latency_samples();
        const uint32_t sample_rate = loader->editor_chrome_sample_rate();
        if (latency > 0 && sample_rate > 0) {
            wchar_t suffix[96]{};
            swprintf_s(suffix,
                       L"  |  %.1f ms",
                       static_cast<double>(latency) * 1000.0 / static_cast<double>(sample_rate));
            title += suffix;
        }
    }
    RECT text_rect{12, 0, close_rect.left - 10, title_rect.bottom};
    DrawTextW(hdc,
              title.c_str(),
              static_cast<int>(title.size()),
              &text_rect,
              DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
    EndPaint(hwnd, &ps);
}

LRESULT CALLBACK BridgeViewContainerProc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    if (msg == WM_NCCREATE) {
        auto* cs = reinterpret_cast<CREATESTRUCTW*>(lparam);
        if (cs) {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(cs->lpCreateParams));
        }
    }
    auto* loader = reinterpret_cast<miv::PluginLoader*>(GetWindowLongPtrW(hwnd, GWLP_USERDATA));
    if (msg == kBridgeResizePluginClientMsg) {
        resize_frame_for_plugin_client(hwnd,
                                       static_cast<int>(wparam),
                                       static_cast<int>(lparam));
        if (loader) {
            layout_editor_child(hwnd, reinterpret_cast<HWND>(loader->gui_plugin_host_hwnd()));
        }
        return 0;
    }
    if (msg == WM_CLOSE) {
        hide_editor_surface_from_user(hwnd, loader);
        return 0;
    }
    if (msg == WM_SYSCOMMAND && ((wparam & 0xFFF0) == SC_CLOSE)) {
        hide_editor_surface_from_user(hwnd, loader);
        return 0;
    }
    if (msg == WM_SIZE && wparam != SIZE_MINIMIZED) {
        if (loader) {
            layout_editor_child(hwnd, reinterpret_cast<HWND>(loader->gui_plugin_host_hwnd()));
            loader->handle_editor_drag_tick(msg);
            loader->handle_editor_window_size();
        }
        RedrawWindow(hwnd,
                     nullptr,
                     nullptr,
                     RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW | RDW_FRAME);
    }
    if (msg == WM_MOUSEACTIVATE) {
        const WORD hit = LOWORD(lparam);
        if (hit == HTCAPTION || hit == HTCLIENT) {
            return MA_NOACTIVATE;
        }
    }
    if (msg == WM_NCHITTEST) {
        POINT pt{GET_X_LPARAM(lparam), GET_Y_LPARAM(lparam)};
        ScreenToClient(hwnd, &pt);
        RECT client{};
        GetClientRect(hwnd, &client);
        UINT dpi = GetDpiForWindow(hwnd);
        if (dpi == 0) dpi = 96;
        const int frame = GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi) +
                          GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
        const bool resizable =
            (GetWindowLongPtrW(hwnd, GWL_STYLE) & WS_THICKFRAME) == WS_THICKFRAME;
        if (resizable) {
            const bool left = pt.x < frame;
            const bool right = pt.x >= client.right - frame;
            const bool top = pt.y < frame;
            const bool bottom = pt.y >= client.bottom - frame;
            if (top && left) return HTTOPLEFT;
            if (top && right) return HTTOPRIGHT;
            if (bottom && left) return HTBOTTOMLEFT;
            if (bottom && right) return HTBOTTOMRIGHT;
            if (left) return HTLEFT;
            if (right) return HTRIGHT;
            if (top) return HTTOP;
            if (bottom) return HTBOTTOM;
        }
        if (point_in_rect(editor_close_button_rect(hwnd), pt)) {
            return HTCLIENT;
        }
        if (pt.y < editor_titlebar_height(hwnd)) {
            return HTCAPTION;
        }
        return HTCLIENT;
    }
    if (msg == WM_LBUTTONDOWN) {
        POINT pt{GET_X_LPARAM(lparam), GET_Y_LPARAM(lparam)};
        if (point_in_rect(editor_close_button_rect(hwnd), pt)) {
            SetCapture(hwnd);
            return 0;
        }
    }
    if (msg == WM_LBUTTONUP) {
        POINT pt{GET_X_LPARAM(lparam), GET_Y_LPARAM(lparam)};
        if (GetCapture() == hwnd) {
            ReleaseCapture();
        }
        if (point_in_rect(editor_close_button_rect(hwnd), pt)) {
            hide_editor_surface_from_user(hwnd, loader);
            return 0;
        }
    }
    if (msg == WM_PAINT) {
        draw_editor_chrome(hwnd, loader);
        return 0;
    }
    if (msg == WM_ENTERSIZEMOVE) {
        if (loader) {
            loader->handle_editor_drag_start();
        }
    }
    if (msg == WM_EXITSIZEMOVE) {
        if (loader) {
            loader->handle_editor_drag_end();
        }
    }
    if ((msg == WM_MOVE || msg == WM_MOVING || msg == WM_WINDOWPOSCHANGED) && loader) {
        loader->handle_editor_drag_tick(msg);
    }
    if (msg == WM_WINDOWPOSCHANGING) {
        auto* wp = reinterpret_cast<WINDOWPOS*>(lparam);
        if (wp && ((wp->flags & SWP_NOMOVE) == 0 || (wp->flags & SWP_NOSIZE) == 0)) {
            if (wp->x < -30000 || wp->y < -30000 || wp->cx <= 1 || wp->cy <= 1) {
                blog("bridge container suspicious WINDOWPOSCHANGING hwnd=0x%llx flags=0x%X x=%d y=%d cx=%d cy=%d",
                     reinterpret_cast<unsigned long long>(hwnd),
                     wp->flags,
                     wp->x,
                     wp->y,
                     wp->cx,
                     wp->cy);
            }
        }
    }
    if (msg == WM_ACTIVATEAPP) {
        // Do not lower the surface directly here. Moving focus between the Rust
        // host HWND and the bridge-owned container can deactivate this process
        // even though the mIV window group is still the foreground UI. The Rust
        // side polls the actual foreground process and sends set_gui_app_active
        // when the group really leaves or re-enters the foreground.
    }
    return DefWindowProcW(hwnd, msg, wparam, lparam);
}

LRESULT CALLBACK BridgePluginHostProc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    if (msg == WM_MOUSEACTIVATE) {
        return MA_NOACTIVATE;
    }
    return DefWindowProcW(hwnd, msg, wparam, lparam);
}

bool ensure_bridge_view_container_class() {
    WNDCLASSEXW wc{};
    wc.cbSize = sizeof(wc);
    wc.lpfnWndProc = BridgeViewContainerProc;
    wc.hInstance = GetModuleHandleW(nullptr);
    wc.hCursor = LoadCursorW(nullptr, IDC_ARROW);
    wc.lpszClassName = kBridgeViewContainerClass;
    if (RegisterClassExW(&wc) != 0) {
        return true;
    }
    return GetLastError() == ERROR_CLASS_ALREADY_EXISTS;
}

bool ensure_bridge_plugin_host_class() {
    WNDCLASSEXW wc{};
    wc.cbSize = sizeof(wc);
    wc.lpfnWndProc = BridgePluginHostProc;
    wc.hInstance = GetModuleHandleW(nullptr);
    wc.hCursor = LoadCursorW(nullptr, IDC_ARROW);
    wc.lpszClassName = kBridgePluginHostClass;
    if (RegisterClassExW(&wc) != 0) {
        return true;
    }
    return GetLastError() == ERROR_CLASS_ALREADY_EXISTS;
}

bool host_client_rect_on_screen(HWND host_hwnd, RECT& out_rect) {
    if (!host_hwnd || !IsWindow(host_hwnd)) {
        return false;
    }
    if (IsIconic(host_hwnd)) {
        return false;
    }
    RECT client{};
    if (!GetClientRect(host_hwnd, &client)) {
        return false;
    }
    POINT origin{0, 0};
    if (!ClientToScreen(host_hwnd, &origin)) {
        return false;
    }
    const int width = std::max<LONG>(1, client.right - client.left);
    const int height = std::max<LONG>(1, client.bottom - client.top);
    RECT candidate{origin.x, origin.y, origin.x + width, origin.y + height};
    HMONITOR monitor = MonitorFromRect(&candidate, MONITOR_DEFAULTTONEAREST);
    MONITORINFO info{};
    info.cbSize = sizeof(info);
    if (!monitor || !GetMonitorInfoW(monitor, &info)) {
        return false;
    }
    RECT clipped{};
    if (!IntersectRect(&clipped, &candidate, &info.rcWork)) {
        return false;
    }
    out_rect = candidate;
    return true;
}

void apply_dark_editor_chrome(HWND hwnd) {
    if (!hwnd) return;
    BOOL dark = TRUE;
    HRESULT hr = DwmSetWindowAttribute(hwnd,
                                       DWMWA_USE_IMMERSIVE_DARK_MODE,
                                       &dark,
                                       sizeof(dark));
    if (FAILED(hr)) {
        // Older Windows 10 SDKs used 19 for the same private attribute.
        constexpr DWORD kCompatDarkModeAttribute = 19;
        DwmSetWindowAttribute(hwnd,
                              kCompatDarkModeAttribute,
                              &dark,
                              sizeof(dark));
    }
    const COLORREF caption = RGB(18, 18, 18);
    const COLORREF text = RGB(230, 230, 230);
    const COLORREF border = RGB(56, 56, 56);
    DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR, &caption, sizeof(caption));
    DwmSetWindowAttribute(hwnd, DWMWA_TEXT_COLOR, &text, sizeof(text));
    DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &border, sizeof(border));
}

struct BridgeEditorWindows {
    HWND frame = nullptr;
    HWND plugin_host = nullptr;
};

BridgeEditorWindows create_bridge_view_container(const miv::GuiWindowOptions& options,
                                                 miv::PluginLoader* loader) {
    HWND owner_hwnd = reinterpret_cast<HWND>(options.owner_hwnd);
    if (!ensure_bridge_view_container_class() || !ensure_bridge_plugin_host_class()) {
        return {};
    }
    const DWORD style = WS_POPUP | WS_BORDER |
                        (options.resizable ? WS_THICKFRAME : 0) |
                        WS_CLIPCHILDREN | WS_CLIPSIBLINGS;
    const DWORD ex_style = WS_EX_TOOLWINDOW | WS_EX_WINDOWEDGE;
    UINT dpi = owner_hwnd ? GetDpiForWindow(owner_hwnd) : GetDpiForSystem();
    if (dpi == 0) dpi = 96;
    const int title_h = std::max(28, MulDiv(34, static_cast<int>(dpi), 96));
    RECT outer{0,
               0,
               static_cast<LONG>(std::max<uint32_t>(1, options.width)),
               static_cast<LONG>(std::max<uint32_t>(1, options.height)) + title_h};
    AdjustWindowRectExForDpi(&outer, style, FALSE, ex_style, dpi);
    const int width = std::max<LONG>(1, outer.right - outer.left);
    const int height = std::max<LONG>(1, outer.bottom - outer.top);
    const int x = options.has_initial_pos ? options.x : CW_USEDEFAULT;
    const int y = options.has_initial_pos ? options.y : CW_USEDEFAULT;
    std::wstring title = utf8_to_wide(options.title);
    HWND container = CreateWindowExW(ex_style,
                                     kBridgeViewContainerClass,
                                     title.c_str(),
                                     style,
                                     x,
                                     y,
                                     width,
                                     height,
                                     owner_hwnd,
                                     nullptr,
                                     GetModuleHandleW(nullptr),
                                     loader);
    if (!container) {
        blog("bridge view container create failed err=%lu", GetLastError());
        return {};
    }
    HWND plugin_host = CreateWindowExW(0,
                                       kBridgePluginHostClass,
                                       L"",
                                       WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                                       0,
                                       title_h,
                                       static_cast<int>(options.width),
                                       static_cast<int>(options.height),
                                       container,
                                       nullptr,
                                       GetModuleHandleW(nullptr),
                                       nullptr);
    if (!plugin_host) {
        blog("bridge plugin host child create failed err=%lu", GetLastError());
        DestroyWindow(container);
        return {};
    }
    layout_editor_child(container, plugin_host);
    apply_dark_editor_chrome(container);
    blog("bridge editor window created gui_tid=%lu frame=0x%llx child=0x%llx owner=0x%llx pos=%d,%d client=%ux%u outer=%dx%d title_h=%d",
         GetCurrentThreadId(),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(container)),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(plugin_host)),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(owner_hwnd)),
         x,
         y,
         options.width,
         options.height,
         width,
         height,
         title_h);
    return {container, plugin_host};
}

}  // namespace

#include "host_app.h"
#include "pluginterfaces/base/funknownimpl.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivstmessage.h"  // IConnectionPoint
#include "pluginterfaces/vst/ivstprocesscontext.h"
#include "pluginterfaces/vst/vsttypes.h"
#include "public.sdk/source/common/memorystream.h"
#include "public.sdk/source/vst/hosting/processdata.h"
#include "public.sdk/source/vst/hosting/eventlist.h"
#include "public.sdk/source/vst/hosting/parameterchanges.h"

namespace miv {

using namespace Steinberg;

void send_event_error(const std::string& detail);

namespace {
constexpr UINT kPluginGuiThreadTaskMsg = WM_APP + 0x4D1;
constexpr auto kGuiQueryTimeout = std::chrono::milliseconds(2000);
constexpr auto kGuiShowTimeout = std::chrono::milliseconds(3000);
constexpr auto kGuiMutationTimeout = std::chrono::milliseconds(1000);
constexpr auto kPluginLoadTimeout = std::chrono::milliseconds(20000);
constexpr auto kPluginStateTimeout = std::chrono::milliseconds(5000);
constexpr DWORD kGuiShutdownTimeoutMs = 2000;

[[noreturn]] void terminate_bridge_after_gui_timeout(const char* label,
                                                     DWORD gui_tid,
                                                     unsigned long long timeout_ms) {
    blog("VST3 bridge poisoned by plugin GUI timeout label=%s gui_tid=%lu timeout_ms=%llu; terminating bridge process",
         label ? label : "(unknown)",
         gui_tid,
         timeout_ms);
    std::string detail = std::string("bridge poisoned: GUI task '") +
                         (label ? label : "unknown") +
                         "' timed out after " + std::to_string(timeout_ms) +
                         "ms (gui_tid=" + std::to_string(gui_tid) + ")";
    send_event_error(detail);
    std::fflush(stdout);
    std::fflush(stderr);
    ::ExitProcess(1);
}
}

class PluginGuiThread {
public:
    PluginGuiThread() = default;
    ~PluginGuiThread() {
        shutdown();
    }

    PluginGuiThread(const PluginGuiThread&) = delete;
    PluginGuiThread& operator=(const PluginGuiThread&) = delete;

    bool is_current_thread() const {
        DWORD tid = thread_id_.load(std::memory_order_acquire);
        return tid != 0 && tid == GetCurrentThreadId();
    }

    DWORD thread_id() const {
        return thread_id_.load(std::memory_order_acquire);
    }

    // A timeout means the slot GUI thread is stuck in plugin code. The bridge
    // process exits from terminate_bridge_after_gui_timeout rather than
    // returning to the caller with a poisoned plugin instance still alive.
    template <typename Fn>
    auto invoke_sync_for(std::chrono::milliseconds timeout,
                         const char* label,
                         Fn&& fn) -> std::optional<decltype(fn())> {
        using R = decltype(fn());
        static_assert(!std::is_void_v<R>, "use invoke_void_sync_for for void tasks");
        if (is_current_thread()) {
            return fn();
        }

        ensure_started();
        auto task = std::make_shared<std::packaged_task<R()>>(std::forward<Fn>(fn));
        auto future = task->get_future();
        post_task([task]() {
            (*task)();
        });
        if (future.wait_for(timeout) != std::future_status::ready) {
            DWORD gui_tid = thread_id_.load(std::memory_order_acquire);
            blog("plugin GUI task timeout label=%s gui_tid=%lu timeout_ms=%llu",
                 label ? label : "(unknown)",
                 gui_tid,
                 static_cast<unsigned long long>(timeout.count()));
            terminate_bridge_after_gui_timeout(
                label,
                gui_tid,
                static_cast<unsigned long long>(timeout.count()));
        }
        return future.get();
    }

    // Same timeout policy as invoke_sync_for: timeouts terminate the bridge.
    bool invoke_void_sync_for(std::chrono::milliseconds timeout,
                              const char* label,
                              std::function<void()> fn) {
        if (is_current_thread()) {
            fn();
            return true;
        }

        ensure_started();
        auto task = std::make_shared<std::packaged_task<void()>>(std::move(fn));
        auto future = task->get_future();
        post_task([task]() {
            (*task)();
        });
        if (future.wait_for(timeout) != std::future_status::ready) {
            DWORD gui_tid = thread_id_.load(std::memory_order_acquire);
            blog("plugin GUI task timeout label=%s gui_tid=%lu timeout_ms=%llu",
                 label ? label : "(unknown)",
                 gui_tid,
                 static_cast<unsigned long long>(timeout.count()));
            terminate_bridge_after_gui_timeout(
                label,
                gui_tid,
                static_cast<unsigned long long>(timeout.count()));
        }
        future.get();
        return true;
    }

    void post_async(std::function<void()> fn) {
        if (is_current_thread()) {
            fn();
            return;
        }
        ensure_started();
        post_task(std::move(fn));
    }

private:
    void ensure_started() {
        if (running_.load(std::memory_order_acquire)) {
            return;
        }

        std::unique_lock<std::mutex> lk(start_mutex_);
        if (running_.load(std::memory_order_acquire)) {
            return;
        }

        ready_ = false;
        thread_ = std::thread([this]() {
            thread_main();
        });
        start_cv_.wait(lk, [this]() {
            return ready_;
        });
    }

    void post_task(std::function<void()> fn) {
        {
            std::lock_guard<std::mutex> lk(queue_mutex_);
            tasks_.push_back(std::move(fn));
        }
        DWORD tid = thread_id_.load(std::memory_order_acquire);
        if (tid != 0) {
            PostThreadMessageW(tid, kPluginGuiThreadTaskMsg, 0, 0);
        }
    }

    void drain_tasks() {
        for (;;) {
            std::function<void()> task;
            {
                std::lock_guard<std::mutex> lk(queue_mutex_);
                if (tasks_.empty()) {
                    break;
                }
                task = std::move(tasks_.front());
                tasks_.pop_front();
            }
            task();
        }
    }

    void thread_main() {
        thread_id_.store(GetCurrentThreadId(), std::memory_order_release);
        running_.store(true, std::memory_order_release);
        HRESULT ole_hr = OleInitialize(nullptr);
        const bool ole_ok = SUCCEEDED(ole_hr);
        if (FAILED(ole_hr)) {
            blog("plugin GUI thread OleInitialize failed tid=%lu hr=0x%lx",
                 GetCurrentThreadId(),
                 static_cast<unsigned long>(ole_hr));
        }

        MSG msg{};
        PeekMessageW(&msg, nullptr, WM_USER, WM_USER, PM_NOREMOVE);
        {
            std::lock_guard<std::mutex> lk(start_mutex_);
            ready_ = true;
        }
        start_cv_.notify_all();

        blog("plugin GUI thread start gui_tid=%lu", GetCurrentThreadId());
        while (GetMessageW(&msg, nullptr, 0, 0) > 0) {
            if (msg.message == kPluginGuiThreadTaskMsg) {
                drain_tasks();
                continue;
            }
            TranslateMessage(&msg);
            const ULONGLONG dispatch_started = GetTickCount64();
            DispatchMessageW(&msg);
            const ULONGLONG dispatch_elapsed = GetTickCount64() - dispatch_started;
            if (dispatch_elapsed >= 100 && msg.message != WM_NCLBUTTONDOWN) {
                blog("slow plugin GUI DispatchMessageW gui_tid=%lu msg=0x%X hwnd=0x%llx elapsed_ms=%llu",
                     GetCurrentThreadId(),
                     msg.message,
                     static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(msg.hwnd)),
                     static_cast<unsigned long long>(dispatch_elapsed));
            }
        }
        drain_tasks();

        running_.store(false, std::memory_order_release);
        thread_id_.store(0, std::memory_order_release);
        if (ole_ok) {
            OleUninitialize();
        }
        blog("plugin GUI thread exit gui_tid=%lu", GetCurrentThreadId());
    }

    void shutdown() {
        DWORD tid = thread_id_.load(std::memory_order_acquire);
        if (tid == 0) {
            if (thread_.joinable() && thread_.get_id() != std::this_thread::get_id()) {
                thread_.join();
            }
            return;
        }
        if (tid == GetCurrentThreadId()) {
            PostThreadMessageW(tid, WM_QUIT, 0, 0);
            if (thread_.joinable()) {
                thread_.detach();
            }
            return;
        }
        PostThreadMessageW(tid, WM_QUIT, 0, 0);
        if (thread_.joinable()) {
            HANDLE native = thread_.native_handle();
            DWORD wait_result = WaitForSingleObject(native, kGuiShutdownTimeoutMs);
            if (wait_result == WAIT_TIMEOUT) {
                blog("plugin GUI thread shutdown: timed out waiting %lums, detaching gui_tid=%lu",
                     static_cast<unsigned long>(kGuiShutdownTimeoutMs),
                     tid);
                thread_.detach();
                thread_id_.store(0, std::memory_order_release);
                running_.store(false, std::memory_order_release);
                return;
            }
            if (wait_result == WAIT_FAILED) {
                blog("plugin GUI thread shutdown: WaitForSingleObject failed gui_tid=%lu err=%lu",
                     tid,
                     GetLastError());
                thread_.detach();
                thread_id_.store(0, std::memory_order_release);
                running_.store(false, std::memory_order_release);
                return;
            }
            thread_.join();
        }
    }

    std::thread thread_;
    std::atomic<DWORD> thread_id_{0};
    std::atomic<bool> running_{false};
    std::mutex start_mutex_;
    std::condition_variable start_cv_;
    bool ready_ = false;
    std::mutex queue_mutex_;
    std::deque<std::function<void()>> tasks_;
};

struct GuiSizeQueryResult {
    bool ok = false;
    uint32_t width = 0;
    uint32_t height = 0;
    bool resizable = false;
};

struct GuiShowResult {
    bool ok = false;
    std::string error;
};

struct LoadResult {
    bool ok = false;
    LoadedPluginInfo info;
    std::string error;
};

PluginLoader::PluginLoader() {
    host_app_ = owned(new HostApplication);
    component_handler_ = owned(new ComponentHandler);
    plug_frame_ = owned(new PlugFrame);
}

PluginLoader::~PluginLoader() {
    unload();
}

// The bridge/control thread owns lazy creation. GUI-thread callers should only
// reach this after the helper already exists.
PluginGuiThread& PluginLoader::gui_thread() {
    if (!gui_thread_) {
        gui_thread_ = std::make_unique<PluginGuiThread>();
    }
    return *gui_thread_;
}

bool PluginLoader::is_gui_thread() const {
    return gui_thread_ && gui_thread_->is_current_thread();
}

void PluginLoader::quarantine_editor(const char* reason) {
    bool was_quarantined = editor_quarantined_.exchange(true, std::memory_order_acq_rel);
    if (!was_quarantined) {
        blog("plugin editor quarantined plugin=\"%s\" reason=%s gui_tid=%lu",
             plugin_name_.empty() ? "(unknown)" : plugin_name_.c_str(),
             reason ? reason : "(unknown)",
             gui_thread_ ? gui_thread_->thread_id() : 0);
    }
}

void PluginLoader::abandon_gui_thread(const char* reason) {
    if (!gui_thread_) {
        return;
    }
    DWORD tid = gui_thread_->thread_id();
    PluginGuiThread* abandoned = gui_thread_.release();
    blog("plugin GUI thread abandoned plugin=\"%s\" reason=%s gui_tid=%lu helper=0x%llx",
         plugin_name_.empty() ? "(unknown)" : plugin_name_.c_str(),
         reason ? reason : "(unknown)",
         tid,
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(abandoned)));
}

bool PluginLoader::probe(const std::string& plugin_path,
                         PluginProbeInfo& info_out,
                         std::string& error_out) {
    // Probe runs in a short-lived metadata process and never creates or
    // attaches an editor view, so it does not need the per-slot GUI thread.
    std::string load_err;
    auto module = VST3::Hosting::Module::create(plugin_path, load_err);
    if (!module) {
        error_out = "Module::create failed: " + load_err;
        return false;
    }

    Steinberg::IPtr<HostApplication> host_app = owned(new HostApplication);
    const auto& factory = module->getFactory();
    bool found_audio_effect = false;
    bool found_usable = false;
    PluginProbeInfo best{};
    for (const auto& info : factory.classInfos()) {
        if (info.category() != kVstAudioEffectClass) {
            continue;
        }

        auto component = factory.createInstance<Vst::IComponent>(info.ID());
        if (!component) {
            continue;
        }
        if (component->initialize(host_app) != kResultOk) {
            component = nullptr;
            continue;
        }

        auto processor = Steinberg::FUnknownPtr<Vst::IAudioProcessor>(component);
        if (!processor) {
            component->terminate();
            component = nullptr;
            continue;
        }

        PluginProbeInfo cur{};
        cur.plugin_name = info.name();
        const int32 audio_in = component->getBusCount(Vst::kAudio, Vst::kInput);
        const int32 audio_out = component->getBusCount(Vst::kAudio, Vst::kOutput);
        const int32 event_in = component->getBusCount(Vst::kEvent, Vst::kInput);
        const int32 event_out = component->getBusCount(Vst::kEvent, Vst::kOutput);
        cur.audio_input_buses = audio_in > 0 ? static_cast<uint32_t>(audio_in) : 0;
        cur.audio_output_buses = audio_out > 0 ? static_cast<uint32_t>(audio_out) : 0;
        cur.event_input_buses = event_in > 0 ? static_cast<uint32_t>(event_in) : 0;
        cur.event_output_buses = event_out > 0 ? static_cast<uint32_t>(event_out) : 0;

        auto sum_channels = [&](Vst::MediaType media, Vst::BusDirection direction,
                                int32 count) -> uint32_t {
            uint32_t total = 0;
            for (int32 i = 0; i < count; ++i) {
                Vst::BusInfo bus{};
                if (component->getBusInfo(media, direction, i, bus) == kResultOk &&
                    bus.channelCount > 0) {
                    total += static_cast<uint32_t>(bus.channelCount);
                }
            }
            return total;
        };
        cur.audio_input_channels = audio_in > 0
            ? sum_channels(Vst::kAudio, Vst::kInput, audio_in)
            : 0;
        cur.audio_output_channels = audio_out > 0
            ? sum_channels(Vst::kAudio, Vst::kOutput, audio_out)
            : 0;
        cur.usable_audio_effect = cur.audio_input_buses > 0 &&
                                  cur.audio_output_buses > 0;

        component->terminate();
        if (!found_audio_effect || (cur.usable_audio_effect && !found_usable)) {
            best = cur;
            found_audio_effect = true;
            found_usable = cur.usable_audio_effect;
        }
    }

    if (found_audio_effect) {
        info_out = best;
        return true;
    }

    error_out = "no AudioEffectClass found in plugin";
    return false;
}

bool PluginLoader::load(const std::string& plugin_path,
                         uint32_t sample_rate,
                         uint32_t block_size,
                         LoadedPluginInfo& info_out,
                         std::string& error_out) {
    if (!is_gui_thread()) {
        std::string path = plugin_path;
        auto result = gui_thread().invoke_sync_for(
            kPluginLoadTimeout,
            "load_plugin",
            [this, path, sample_rate, block_size]() -> LoadResult {
                LoadResult out{};
                out.ok = load(path, sample_rate, block_size, out.info, out.error);
                return out;
            });
        if (!result) {
            error_out = "load_plugin timed out";
            return false;
        }
        if (!result->ok) {
            error_out = result->error;
            return false;
        }
        info_out = result->info;
        return true;
    }

    sample_rate_ = sample_rate;
    block_size_ = block_size;
    blog("load: plugin lifecycle on gui_tid=%lu path=\"%s\"",
         GetCurrentThreadId(),
         plugin_path.c_str());

    // VST3 SDK の Module ヘルパで .vst3 をロード
    std::string load_err;
    module_ = VST3::Hosting::Module::create(plugin_path, load_err);
    if (!module_) {
        error_out = "Module::create failed: " + load_err;
        return false;
    }

    // Factory から最初の AudioProcessor クラスを探す
    const auto& factory = module_->getFactory();
    for (const auto& info : factory.classInfos()) {
        if (info.category() == kVstAudioEffectClass) {
            // IComponent をインスタンス化
            component_ = factory.createInstance<Vst::IComponent>(info.ID());
            if (!component_) {
                continue;
            }
            // ホストとして初期化
            if (component_->initialize(host_app_) != kResultOk) {
                component_ = nullptr;
                continue;
            }
            // IAudioProcessor を取得
            processor_ = Steinberg::FUnknownPtr<Vst::IAudioProcessor>(component_);
            if (!processor_) {
                component_->terminate();
                component_ = nullptr;
                continue;
            }
            // IEditController は同一クラスから来る場合もある
            controller_ = Steinberg::FUnknownPtr<Vst::IEditController>(component_);
            if (!controller_) {
                // 別クラスとして提供されることもある (= TUID で controller class を取って createInstance)
                TUID ctrl_cid;
                if (component_->getControllerClassId(ctrl_cid) == kResultOk) {
                    controller_ = factory.createInstance<Vst::IEditController>(VST3::UID::fromTUID(ctrl_cid));
                    if (controller_) {
                        controller_->initialize(host_app_);
                    }
                }
            }
            if (controller_) {
                controller_->setComponentHandler(component_handler_);

                // VST3 必須: IComponent と IEditController を IConnectionPoint で
                // 接続し、component の state を controller に同期する。
                // これが無いと一部のプラグイン (Pro-Q 4 等) は UI 操作が音声処理に
                // 反映されず、内部アナライザも動作しない。
                auto component_cp = Steinberg::FUnknownPtr<Steinberg::Vst::IConnectionPoint>(component_);
                auto controller_cp = Steinberg::FUnknownPtr<Steinberg::Vst::IConnectionPoint>(controller_);
                if (component_cp && controller_cp) {
                    component_cp->connect(controller_cp);
                    controller_cp->connect(component_cp);
                    blog("load: component <-> controller connected");
                }

                // component の state を controller にコピー (= デフォルトパラメータ等)。
                Steinberg::MemoryStream stream;
                if (component_->getState(&stream) == Steinberg::kResultOk) {
                    stream.seek(0, Steinberg::IBStream::kIBSeekSet, nullptr);
                    if (controller_->setComponentState(&stream) == Steinberg::kResultOk) {
                        blog("load: setComponentState ok");
                    }
                }
            }

            info_out.plugin_name = info.name();
            plugin_name_ = info_out.plugin_name;
            break;
        }
    }

    if (!component_ || !processor_) {
        error_out = "no AudioEffectClass found in plugin";
        unload();
        return false;
    }

    // Bus 設定: 動的に取得した bus 数に合わせて arrangement を渡す。
    // Pro-Q 4 等、サイドチェイン入力 bus を持つプラグインは 1 bus だけ
    // 渡すと kResultFalse を返すため、全 audio bus 分を埋める必要がある。
    //
    // 戦略:
    //   1) 全 bus を stereo で埋めて setBusArrangements
    //   2) 失敗したら副 bus を空 (= mono など最小) にしてリトライ
    //   3) それでも失敗すれば諦める
    int32 num_in_buses = component_->getBusCount(Vst::kAudio, Vst::kInput);
    int32 num_out_buses = component_->getBusCount(Vst::kAudio, Vst::kOutput);
    if (num_in_buses < 1 || num_out_buses < 1) {
        error_out = "plugin has no audio bus";
        unload();
        return false;
    }

    auto try_arrangements = [&](Vst::SpeakerArrangement aux) -> bool {
        std::vector<Vst::SpeakerArrangement> ins(num_in_buses, Vst::SpeakerArr::kStereo);
        std::vector<Vst::SpeakerArrangement> outs(num_out_buses, Vst::SpeakerArr::kStereo);
        for (int32 i = 1; i < num_in_buses; ++i) ins[i] = aux;
        for (int32 i = 1; i < num_out_buses; ++i) outs[i] = aux;
        return processor_->setBusArrangements(
                   ins.data(), num_in_buses, outs.data(), num_out_buses) == kResultOk;
    };

    bool arr_ok = try_arrangements(Vst::SpeakerArr::kStereo);
    if (!arr_ok) {
        // サイドチェインを mono で
        arr_ok = try_arrangements(Vst::SpeakerArr::kMono);
    }
    if (!arr_ok) {
        // サイドチェインを空 (= 無効化) で
        arr_ok = try_arrangements(Vst::SpeakerArr::kEmpty);
    }
    if (!arr_ok) {
        error_out = "setBusArrangements failed for stereo main bus (in="
                    + std::to_string(num_in_buses) + " out="
                    + std::to_string(num_out_buses) + ")";
        unload();
        return false;
    }

    // main bus (index 0) のみ active にし、副 bus は無効にして処理経路から外す
    for (int32 i = 0; i < num_in_buses; ++i) {
        component_->activateBus(Vst::kAudio, Vst::kInput, i, i == 0);
    }
    for (int32 i = 0; i < num_out_buses; ++i) {
        component_->activateBus(Vst::kAudio, Vst::kOutput, i, i == 0);
    }

    // ProcessSetup
    Vst::ProcessSetup setup{};
    setup.processMode = Vst::kRealtime;
    setup.symbolicSampleSize = Vst::kSample32;
    setup.maxSamplesPerBlock = static_cast<int32>(block_size);
    setup.sampleRate = static_cast<double>(sample_rate);
    if (processor_->setupProcessing(setup) != kResultOk) {
        error_out = "setupProcessing failed";
        unload();
        return false;
    }

    // Activate
    if (component_->setActive(true) != kResultOk) {
        error_out = "setActive(true) failed";
        unload();
        return false;
    }
    processor_->setProcessing(true);
    active_ = true;

    cached_latency_samples_.store(
        static_cast<uint32_t>(processor_->getLatencySamples()),
        std::memory_order_release);
    info_out.latency_samples = cached_latency_samples_.load(std::memory_order_acquire);

    // 事前確保: planar バッファ (各 channel あたり block_size sample)
    in_buffer_l_.resize(block_size);
    in_buffer_r_.resize(block_size);
    out_buffer_l_.resize(block_size);
    out_buffer_r_.resize(block_size);
    // 副 bus 用 silence (サイドチェインに無音を流すため)
    dummy_in_buf_.assign(block_size, 0.0f);
    dummy_out_buf_.assign(block_size, 0.0f);
    // process_block で参照する bus 数
    num_in_buses_ = num_in_buses;
    num_out_buses_ = num_out_buses;

    return true;
}

bool PluginLoader::process_block(const float* input, float* output, uint32_t num_frames) {
    if (!active_ || !processor_) return false;
    if (num_frames > block_size_) return false;

    // f32 packed stereo → planar に分解。同時に silence 検出も行う。
    bool ch0_silent = true;
    bool ch1_silent = true;
    for (uint32_t i = 0; i < num_frames; ++i) {
        float l = input[i * 2 + 0];
        float r = input[i * 2 + 1];
        in_buffer_l_[i] = l;
        in_buffer_r_[i] = r;
        if (l != 0.0f) ch0_silent = false;
        if (r != 0.0f) ch1_silent = false;
    }

    // ProcessData セットアップ — VST3 仕様により ProcessData::numInputs/numOutputs
    // は **getBusCount で得た値と一致** させる必要がある。Pro-Q 4 等サイドチェイン
    // 入力 bus を持つプラグインに 1 個だけ渡すと UB → 音声が届いていないように
    // 見える原因になる。
    std::vector<Vst::AudioBusBuffers> in_buses(num_in_buses_);
    std::vector<Vst::AudioBusBuffers> out_buses(num_out_buses_);

    // main bus
    float* main_in_planar[2] = { in_buffer_l_.data(), in_buffer_r_.data() };
    float* main_out_planar[2] = { out_buffer_l_.data(), out_buffer_r_.data() };
    in_buses[0].numChannels = 2;
    in_buses[0].channelBuffers32 = main_in_planar;
    // 入力が無音なら silenceFlags を立ててプラグインの silence skip 最適化を許可。
    // bit i = ch i が無音。常時 0 にしているとプラグインが毎ブロック処理を走らせて
    // CPU を浪費しバッファアンダーラン (= プチプチ) を誘発する。
    in_buses[0].silenceFlags = (ch0_silent ? 0x1 : 0) | (ch1_silent ? 0x2 : 0);
    out_buses[0].numChannels = 2;
    out_buses[0].channelBuffers32 = main_out_planar;
    // 出力 silenceFlags はプラグインが書き換えてくる
    out_buses[0].silenceFlags = 0;

    // 副 bus (サイドチェイン等): silence buffer + silenceFlags 全立て
    float* dummy_in_planar[2] = { dummy_in_buf_.data(), dummy_in_buf_.data() };
    float* dummy_out_planar[2] = { dummy_out_buf_.data(), dummy_out_buf_.data() };
    for (int32 i = 1; i < num_in_buses_; ++i) {
        in_buses[i].numChannels = 2;
        in_buses[i].channelBuffers32 = dummy_in_planar;
        in_buses[i].silenceFlags = 0x3;  // ch0+ch1 silent
    }
    for (int32 i = 1; i < num_out_buses_; ++i) {
        out_buses[i].numChannels = 2;
        out_buses[i].channelBuffers32 = dummy_out_planar;
        out_buses[i].silenceFlags = 0x3;
    }

    Vst::ProcessContext ctx{};
    // 時間進行を伝えるフラグも立てる。立てないと Pro-Q 4 のアナライザ等は
    // 時刻情報を信用せずアナライザ更新を停止する。
    ctx.state = Vst::ProcessContext::kPlaying
              | Vst::ProcessContext::kContTimeValid
              | Vst::ProcessContext::kProjectTimeMusicValid
              | Vst::ProcessContext::kTempoValid;
    ctx.sampleRate = static_cast<double>(sample_rate_);
    ctx.projectTimeSamples = process_time_samples_;
    ctx.continousTimeSamples = process_time_samples_;
    ctx.projectTimeMusic = static_cast<double>(process_time_samples_) /
                            static_cast<double>(sample_rate_) *
                            (120.0 / 60.0); // 120 BPM 想定の quarter note 数
    ctx.tempo = 120.0;
    process_time_samples_ += static_cast<int64_t>(num_frames);

    Vst::EventList input_events;
    Vst::EventList output_events;
    Vst::ParameterChanges input_params;
    Vst::ParameterChanges output_params;

    // UI スレッドから蓄積された param 変更 (例: EQ カーブ操作) を取り出して
    // input_params に積む。これが無いと UI 操作が音声処理に反映されない。
    if (component_handler_) {
        component_handler_->drain_into(&input_params);
    }

    Vst::ProcessData data{};
    data.processMode = Vst::kRealtime;
    data.symbolicSampleSize = Vst::kSample32;
    data.numSamples = static_cast<int32>(num_frames);
    data.numInputs = num_in_buses_;
    data.numOutputs = num_out_buses_;
    data.inputs = in_buses.data();
    data.outputs = out_buses.data();
    data.inputParameterChanges = &input_params;
    data.outputParameterChanges = &output_params;
    data.inputEvents = &input_events;
    data.outputEvents = &output_events;
    data.processContext = &ctx;

    if (processor_->process(data) != kResultOk) {
        return false;
    }

    // planar → packed stereo に戻す
    for (uint32_t i = 0; i < num_frames; ++i) {
        output[i * 2 + 0] = out_buffer_l_[i];
        output[i * 2 + 1] = out_buffer_r_[i];
    }
    return true;
}

bool PluginLoader::get_gui_size(uint32_t& width_out, uint32_t& height_out) {
    if (editor_quarantined_.load(std::memory_order_acquire) && !is_gui_thread()) {
        return false;
    }
    if (!is_gui_thread()) {
        auto result = gui_thread().invoke_sync_for(kGuiQueryTimeout, "get_gui_size", [this]() {
            GuiSizeQueryResult out{};
            out.ok = get_gui_size(out.width, out.height);
            return out;
        });
        if (!result || !result->ok) {
            if (!result) {
                quarantine_editor("get_gui_size timeout");
                abandon_gui_thread("get_gui_size timeout");
            }
            return false;
        }
        width_out = result->width;
        height_out = result->height;
        return true;
    }

    // 既存 view_ から純粋に getSize する (= scale 設定を変更しない)。
    // 既に show_gui で setContentScaleFactor 済みであれば、その scale 込みの
    // 物理ピクセル値を返す。
    // view_ が無ければ一時 view を作って素のサイズを取得 (scale 1.0 想定)。
    if (!controller_) return false;
    Steinberg::IPtr<Steinberg::IPlugView> v = view_;
    if (!v) {
        v = Steinberg::owned(controller_->createView(Steinberg::Vst::ViewType::kEditor));
        if (!v) return false;
    }
    Steinberg::ViewRect rect{};
    if (v->getSize(&rect) != Steinberg::kResultOk) {
        return false;
    }
    int32_t w = rect.right - rect.left;
    int32_t h = rect.bottom - rect.top;
    if (w <= 0 || h <= 0) return false;
    width_out = static_cast<uint32_t>(w);
    height_out = static_cast<uint32_t>(h);
    return true;
}

bool PluginLoader::query_gui_size_at_dpi(uint32_t dpi, uint32_t& width_out, uint32_t& height_out,
                                          bool& resizable_out) {
    if (editor_quarantined_.load(std::memory_order_acquire) && !is_gui_thread()) {
        return false;
    }
    if (!is_gui_thread()) {
        auto result = gui_thread().invoke_sync_for(kGuiQueryTimeout, "query_gui_size_at_dpi", [this, dpi]() {
            GuiSizeQueryResult out{};
            out.ok = query_gui_size_at_dpi(dpi, out.width, out.height, out.resizable);
            return out;
        });
        if (!result || !result->ok) {
            if (!result) {
                quarantine_editor("query_gui_size_at_dpi timeout");
                abandon_gui_thread("query_gui_size_at_dpi timeout");
            }
            return false;
        }
        width_out = result->width;
        height_out = result->height;
        resizable_out = result->resizable;
        return true;
    }

    // 一時 view を作り、指定 DPI の scale を伝えてから getSize → 破棄。
    // ホストウィンドウを作る前に正しいサイズと resizable 属性を知るためのクエリ用。
    if (!controller_) return false;
    auto v = Steinberg::owned(controller_->createView(Steinberg::Vst::ViewType::kEditor));
    if (!v) return false;
    Steinberg::FUnknownPtr<Steinberg::IPlugViewContentScaleSupport> css(v);
    if (css) {
        if (dpi == 0) dpi = 96;
        float factor = static_cast<float>(dpi) / 96.0f;
        css->setContentScaleFactor(factor);
        blog("query_gui_size_at_dpi: setContentScaleFactor=%.3f (dpi=%u)", factor, dpi);
    }
    Steinberg::ViewRect rect{};
    if (v->getSize(&rect) != Steinberg::kResultOk) return false;
    int32_t w = rect.right - rect.left;
    int32_t h = rect.bottom - rect.top;
    if (w <= 0 || h <= 0) return false;
    width_out = static_cast<uint32_t>(w);
    height_out = static_cast<uint32_t>(h);
    // canResize は IPlugView 規約: kResultTrue = サイズ変更可、kResultFalse = 不可。
    // SSL Meter Pro 等の固定サイズ プラグインは false を返すので、ホスト側で
    // WS_THICKFRAME を外して外側ウィンドウのリサイズ自体を禁止する。
    resizable_out = (v->canResize() == Steinberg::kResultTrue);
    return true;
}

bool PluginLoader::show_gui(const GuiWindowOptions& options, bool visible, std::string& error_out) {
    if (editor_quarantined_.load(std::memory_order_acquire) && !is_gui_thread()) {
        error_out = "editor quarantined after a previous GUI timeout";
        return false;
    }
    if (!is_gui_thread()) {
        GuiWindowOptions options_copy = options;
        auto result = gui_thread().invoke_sync_for(
            kGuiShowTimeout, "show_gui", [this, options_copy, visible]() mutable {
                GuiShowResult out{};
                out.ok = show_gui(options_copy, visible, out.error);
                return out;
            });
        if (!result) {
            quarantine_editor("show_gui timeout");
            abandon_gui_thread("show_gui timeout");
            error_out = "show_gui timed out (slot GUI thread stuck in plugin editor attach)";
            return false;
        }
        error_out = result->error;
        return result->ok;
    }

    blog("show_gui start gui_tid=%lu owner=0x%llx visible=%d",
         GetCurrentThreadId(),
         (unsigned long long)options.owner_hwnd,
         visible ? 1 : 0);
    editor_slot_id_ = options.slot_id;
    if (!controller_) {
        error_out = "controller not available";
        return false;
    }
    if (view_attached_) {
        // すでにアタッチ済みなら一度外して付け直す
        blog("show_gui: already attached, hiding first");
        hide_gui();
    }
    if (!view_) {
        blog("show_gui: createView(kEditor)");
        view_ = Steinberg::owned(controller_->createView(Steinberg::Vst::ViewType::kEditor));
        if (!view_) {
            error_out = "createView returned null (no editor)";
            return false;
        }
        blog("show_gui: createView ok");
    }
    // VST3 の HWND タイプは "HWND" 文字列で指定 (kPlatformTypeHWND)
    blog("show_gui: isPlatformTypeSupported(HWND)");
    if (view_->isPlatformTypeSupported(Steinberg::kPlatformTypeHWND) != Steinberg::kResultTrue) {
        error_out = "plugin view does not support HWND platform";
        view_ = nullptr;
        return false;
    }
    blog("show_gui: setFrame");
    // attached より **前に** setFrame を呼ぶ。Pro-Q 4 等多くのプラグインは
    // frame が無いと描画開始しない (= 真っ白でハング)。
    view_->setFrame(plug_frame_);

    // DPI scale をプラグインに伝える。これが無いとプラグインは "100% 想定" で
    // 描画してしまい、Per-Monitor v2 環境で位置/サイズがずれる
    // (Pro-Q 4 で「右下しか見えない」現象の原因)。
    Steinberg::FUnknownPtr<Steinberg::IPlugViewContentScaleSupport> css(view_);
    if (css) {
        UINT dpi = 0;
        HWND owner_hwnd = reinterpret_cast<HWND>(options.owner_hwnd);
        if (owner_hwnd) {
            dpi = GetDpiForWindow(owner_hwnd);
        }
        if (dpi == 0) dpi = GetDpiForSystem();
        if (dpi == 0) dpi = 96;
        float factor = static_cast<float>(dpi) / 96.0f;
        css->setContentScaleFactor(factor);
        blog("show_gui: setContentScaleFactor=%.3f (dpi=%u)", factor, dpi);
    } else {
        blog("show_gui: plugin does not implement IPlugViewContentScaleSupport");
    }

    BridgeEditorWindows editor_hwnds = create_bridge_view_container(options, this);
    HWND frame_hwnd = editor_hwnds.frame;
    HWND attach_hwnd = editor_hwnds.plugin_host;
    if (!frame_hwnd || !attach_hwnd) {
        error_out = "failed to create editor window";
        view_->setFrame(nullptr);
        view_ = nullptr;
        return false;
    }

    plug_frame_->set_host_hwnd(frame_hwnd);
    blog("show_gui: attached(hwnd, HWND) gui_tid=%lu frame=0x%llx child=0x%llx",
         GetCurrentThreadId(),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(frame_hwnd)),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(attach_hwnd)));
    if (view_->attached(attach_hwnd, Steinberg::kPlatformTypeHWND) != Steinberg::kResultOk) {
        error_out = "attached() failed";
        if (IsWindow(frame_hwnd)) {
            DestroyWindow(frame_hwnd);
        }
        view_->setFrame(nullptr);
        view_ = nullptr;
        return false;
    }
    view_attached_ = true;
    view_host_hwnd_ = options.owner_hwnd;
    view_container_hwnd_ = frame_hwnd;
    view_plugin_host_hwnd_ = attach_hwnd;
    editor_slot_id_ = options.slot_id;
    view_container_hwnd_snapshot_.store(frame_hwnd, std::memory_order_release);
    blog("show_gui: attached ok");

    // attached 後に推奨サイズで onSize を呼んで「このサイズで描画して」と通知する。
    // これも描画開始トリガとして必要なプラグインが多い。
    Steinberg::ViewRect rect{};
    if (view_->getSize(&rect) == Steinberg::kResultOk) {
        const int preferred_w = std::max<Steinberg::int32>(1, rect.right - rect.left);
        const int preferred_h = std::max<Steinberg::int32>(1, rect.bottom - rect.top);
        blog("show_gui: getSize=%dx%d, resize container, onSize", preferred_w, preferred_h);
        if (HWND container_hwnd = reinterpret_cast<HWND>(view_container_hwnd_);
            container_hwnd && IsWindow(container_hwnd)) {
            resize_frame_for_plugin_client(container_hwnd, preferred_w, preferred_h);
            layout_editor_child(container_hwnd, reinterpret_cast<HWND>(view_plugin_host_hwnd_));
        }
        view_->onSize(&rect);
        last_gui_width_ = static_cast<uint32_t>(preferred_w);
        last_gui_height_ = static_cast<uint32_t>(preferred_h);
        blog("show_gui: onSize done");
    } else {
        last_gui_width_ = 0;
        last_gui_height_ = 0;
        blog("show_gui: getSize failed");
    }
    gui_surface_visible_ = visible;
    gui_app_active_ = true;
    if (HWND container_hwnd = reinterpret_cast<HWND>(view_container_hwnd_);
        container_hwnd && IsWindow(container_hwnd)) {
        ShowWindow(container_hwnd, (visible && gui_app_active_) ? SW_SHOWNOACTIVATE : SW_HIDE);
        if (visible && gui_app_active_) {
            refresh_gui_surface(container_hwnd);
        }
    }
    blog("show_gui done visible=%d", visible ? 1 : 0);
    return true;
}

void PluginLoader::set_user_resizing(bool active) {
    if (editor_quarantined_.load(std::memory_order_acquire) && !is_gui_thread()) {
        return;
    }
    if (!is_gui_thread()) {
        if (!gui_thread_) {
            return;
        }
        gui_thread().post_async([this, active]() {
            set_user_resizing(active);
        });
        return;
    }

    if (plug_frame_) {
        plug_frame_->set_user_resizing(active);
    }
}

void PluginLoader::refresh_gui_surface(void* container_hwnd_ptr) {
    HWND container_hwnd = reinterpret_cast<HWND>(container_hwnd_ptr);
    if (!container_hwnd || !IsWindow(container_hwnd)) {
        return;
    }
    HWND plugin_hwnd = reinterpret_cast<HWND>(view_plugin_host_hwnd_);
    layout_editor_child(container_hwnd, plugin_hwnd);
    if (view_attached_ && view_) {
        RECT client{};
        if (plugin_hwnd && IsWindow(plugin_hwnd) && GetClientRect(plugin_hwnd, &client)) {
            const uint32_t width = static_cast<uint32_t>(std::max<LONG>(1, client.right - client.left));
            const uint32_t height = static_cast<uint32_t>(std::max<LONG>(1, client.bottom - client.top));
            Steinberg::ViewRect rect{0,
                                     0,
                                     static_cast<Steinberg::int32>(width),
                                     static_cast<Steinberg::int32>(height)};
            view_->onSize(&rect);
            last_gui_width_ = width;
            last_gui_height_ = height;
        }
    }
    RedrawWindow(container_hwnd,
                 nullptr,
                 nullptr,
                 RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_UPDATENOW | RDW_FRAME);
}

void PluginLoader::set_gui_surface_visible_state(bool visible) {
    if (editor_quarantined_.load(std::memory_order_acquire) && !is_gui_thread()) {
        return;
    }
    if (!is_gui_thread()) {
        if (!gui_thread_) {
            return;
        }
        gui_thread().post_async([this, visible]() {
            set_gui_surface_visible_state(visible);
        });
        return;
    }

    gui_surface_visible_ = visible;
}

bool PluginLoader::gui_surface_should_show() {
    if (editor_quarantined_.load(std::memory_order_acquire) && !is_gui_thread()) {
        return false;
    }
    if (!is_gui_thread()) {
        if (!gui_thread_) {
            return false;
        }
        auto result = gui_thread().invoke_sync_for(kGuiQueryTimeout, "gui_surface_should_show", [this]() {
            return gui_surface_should_show();
        });
        return result.value_or(false);
    }

    return gui_surface_visible_ && gui_app_active_;
}

bool PluginLoader::gui_surface_target_rect(int32_t& x_out,
                                           int32_t& y_out,
                                           int32_t& width_out,
                                           int32_t& height_out) {
    if (editor_quarantined_.load(std::memory_order_acquire) && !is_gui_thread()) {
        return false;
    }
    if (!is_gui_thread()) {
        HWND hwnd = reinterpret_cast<HWND>(
            view_container_hwnd_snapshot_.load(std::memory_order_acquire));
        RECT rect{};
        if (!hwnd || !IsWindow(hwnd) || !GetWindowRect(hwnd, &rect)) {
            return false;
        }
        x_out = rect.left;
        y_out = rect.top;
        width_out = std::max<LONG>(1, rect.right - rect.left);
        height_out = std::max<LONG>(1, rect.bottom - rect.top);
        return true;
    }

    RECT rect{};
    HWND container_hwnd = reinterpret_cast<HWND>(view_container_hwnd_);
    if (!container_hwnd || !IsWindow(container_hwnd) || !GetWindowRect(container_hwnd, &rect)) {
        return false;
    }
    x_out = rect.left;
    y_out = rect.top;
    width_out = std::max<LONG>(1, rect.right - rect.left);
    height_out = std::max<LONG>(1, rect.bottom - rect.top);
    return true;
}

void PluginLoader::refresh_gui_surface_now() {
    if (editor_quarantined_.load(std::memory_order_acquire) && !is_gui_thread()) {
        return;
    }
    if (!is_gui_thread()) {
        if (!gui_thread_) {
            return;
        }
        gui_thread().post_async([this]() {
            refresh_gui_surface_now();
        });
        return;
    }

    refresh_gui_surface(view_container_hwnd_);
}

void* PluginLoader::gui_container_hwnd() const {
    return view_container_hwnd_snapshot_.load(std::memory_order_acquire);
}

void PluginLoader::set_gui_visible(bool visible) {
    if (editor_quarantined_.load(std::memory_order_acquire) && !is_gui_thread()) {
        return;
    }
    if (!is_gui_thread()) {
        if (!gui_thread_) {
            return;
        }
        gui_thread().post_async([this, visible]() {
            set_gui_visible(visible);
        });
        return;
    }

    gui_surface_visible_ = visible;
    HWND container_hwnd = reinterpret_cast<HWND>(view_container_hwnd_);
    if (container_hwnd && IsWindow(container_hwnd)) {
        ShowWindow(container_hwnd, (visible && gui_app_active_) ? SW_SHOWNA : SW_HIDE);
        if (visible && gui_app_active_) {
            refresh_gui_surface(container_hwnd);
        }
        blog("set_gui_visible: bridge surface hwnd=0x%llx visible=%d",
             static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(container_hwnd)),
             visible ? 1 : 0);
        return;
    }
    blog("set_gui_visible: no bridge surface visible=%d", visible ? 1 : 0);
}

void PluginLoader::set_gui_topmost(bool topmost) {
    if (editor_quarantined_.load(std::memory_order_acquire) && !is_gui_thread()) {
        return;
    }
    if (!is_gui_thread()) {
        if (!gui_thread_) {
            return;
        }
        gui_thread().post_async([this, topmost]() {
            set_gui_topmost(topmost);
        });
        return;
    }

    HWND container_hwnd = reinterpret_cast<HWND>(view_container_hwnd_);
    if (container_hwnd && IsWindow(container_hwnd)) {
        SetWindowPos(container_hwnd,
                     topmost ? HWND_TOPMOST : HWND_NOTOPMOST,
                     0,
                     0,
                     0,
                     0,
                     SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
        blog("set_gui_topmost: bridge surface hwnd=0x%llx topmost=%d",
             static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(container_hwnd)),
             topmost ? 1 : 0);
        return;
    }
    blog("set_gui_topmost: no bridge surface topmost=%d", topmost ? 1 : 0);
}

void PluginLoader::set_gui_owner(void* owner_hwnd) {
    if (editor_quarantined_.load(std::memory_order_acquire) && !is_gui_thread()) {
        return;
    }
    if (!is_gui_thread()) {
        if (!gui_thread_) {
            return;
        }
        gui_thread().post_async([this, owner_hwnd]() {
            set_gui_owner(owner_hwnd);
        });
        return;
    }

    HWND container_hwnd = reinterpret_cast<HWND>(view_container_hwnd_);
    HWND new_owner = reinterpret_cast<HWND>(owner_hwnd);
    if (!container_hwnd || !IsWindow(container_hwnd) || !new_owner || !IsWindow(new_owner)) {
        blog("set_gui_owner: skipped surface=0x%llx owner=0x%llx",
             static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(container_hwnd)),
             static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(new_owner)));
        return;
    }
    HWND old_owner = reinterpret_cast<HWND>(GetWindowLongPtrW(container_hwnd, GWLP_HWNDPARENT));
    if (old_owner == new_owner) {
        view_host_hwnd_ = owner_hwnd;
        return;
    }
    SetWindowLongPtrW(container_hwnd, GWLP_HWNDPARENT, reinterpret_cast<LONG_PTR>(new_owner));
    SetWindowPos(container_hwnd,
                 nullptr,
                 0,
                 0,
                 0,
                 0,
                 SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
    view_host_hwnd_ = owner_hwnd;
    blog("set_gui_owner: surface=0x%llx owner=0x%llx",
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(container_hwnd)),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(new_owner)));
}

void PluginLoader::set_gui_app_active(bool active) {
    if (editor_quarantined_.load(std::memory_order_acquire) && !is_gui_thread()) {
        return;
    }
    if (!is_gui_thread()) {
        if (!gui_thread_) {
            return;
        }
        gui_thread().post_async([this, active]() {
            set_gui_app_active(active);
        });
        return;
    }

    gui_app_active_ = active;
    HWND container_hwnd = reinterpret_cast<HWND>(view_container_hwnd_);
    if (container_hwnd && IsWindow(container_hwnd)) {
        if (gui_surface_visible_ && gui_app_active_) {
            ShowWindow(container_hwnd, SW_SHOWNA);
            refresh_gui_surface(container_hwnd);
        } else if (gui_surface_visible_) {
            // Do not hide here: several D3D-backed plugin editors repaint as a
            // blank surface after repeated ShowWindow(SW_HIDE/SW_SHOWNA). It is
            // enough to leave the topmost band. Do not send it to HWND_BOTTOM:
            // clicking the Rust host can temporarily deactivate the bridge
            // process while the mIV window group is still foreground, and
            // bottoming the surface makes the editor appear to vanish behind
            // the video.
            SetWindowPos(container_hwnd,
                         HWND_NOTOPMOST,
                         0,
                         0,
                         0,
                         0,
                         SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
        } else {
            ShowWindow(container_hwnd, SW_HIDE);
        }
        blog("set_gui_app_active: bridge surface hwnd=0x%llx active=%d visible=%d",
             static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(container_hwnd)),
             active ? 1 : 0,
             gui_surface_visible_ ? 1 : 0);
        return;
    }
    blog("set_gui_app_active: no bridge surface active=%d", active ? 1 : 0);
}

void PluginLoader::handle_editor_window_size() {
    if (editor_quarantined_.load(std::memory_order_acquire) && !is_gui_thread()) {
        return;
    }
    if (!is_gui_thread()) {
        if (!gui_thread_) {
            return;
        }
        gui_thread().post_async([this]() {
            handle_editor_window_size();
        });
        return;
    }

    HWND container_hwnd = reinterpret_cast<HWND>(view_container_hwnd_);
    if (!view_attached_ || !view_ || !container_hwnd || !IsWindow(container_hwnd)) {
        return;
    }
    HWND plugin_hwnd = reinterpret_cast<HWND>(view_plugin_host_hwnd_);
    layout_editor_child(container_hwnd, plugin_hwnd);
    RECT client{};
    if (!plugin_hwnd || !IsWindow(plugin_hwnd) || !GetClientRect(plugin_hwnd, &client)) {
        return;
    }
    const uint32_t width = static_cast<uint32_t>(std::max<LONG>(1, client.right - client.left));
    const uint32_t height = static_cast<uint32_t>(std::max<LONG>(1, client.bottom - client.top));
    uint32_t constrained_width = width;
    uint32_t constrained_height = height;
    Steinberg::ViewRect constrained_rect{0,
                                         0,
                                         static_cast<Steinberg::int32>(width),
                                         static_cast<Steinberg::int32>(height)};
    view_->checkSizeConstraint(&constrained_rect);
    if (constrained_rect.right > constrained_rect.left &&
        constrained_rect.bottom > constrained_rect.top) {
        constrained_width = static_cast<uint32_t>(std::max<Steinberg::int32>(
            1, constrained_rect.right - constrained_rect.left));
        constrained_height = static_cast<uint32_t>(std::max<Steinberg::int32>(
            1, constrained_rect.bottom - constrained_rect.top));
    }
    if ((constrained_width != width || constrained_height != height) &&
        resize_frame_for_plugin_client(container_hwnd,
                                       static_cast<int>(constrained_width),
                                       static_cast<int>(constrained_height))) {
        layout_editor_child(container_hwnd, plugin_hwnd);
        if (!GetClientRect(plugin_hwnd, &client)) {
            return;
        }
        constrained_width = static_cast<uint32_t>(std::max<LONG>(1, client.right - client.left));
        constrained_height = static_cast<uint32_t>(std::max<LONG>(1, client.bottom - client.top));
        blog("editor host resize constrained plugin=\"%s\" requested=%ux%u constrained=%ux%u",
             plugin_name_.empty() ? "(unknown)" : plugin_name_.c_str(),
             width,
             height,
             constrained_width,
             constrained_height);
    }
    if (constrained_width == last_gui_width_ && constrained_height == last_gui_height_) {
        return;
    }
    last_gui_width_ = constrained_width;
    last_gui_height_ = constrained_height;
    if (plug_frame_) {
        plug_frame_->mark_user_resize();
    }
    Steinberg::ViewRect rect{0,
                             0,
                             static_cast<Steinberg::int32>(constrained_width),
                             static_cast<Steinberg::int32>(constrained_height)};
    view_->onSize(&rect);
    RedrawWindow(container_hwnd,
                 nullptr,
                 nullptr,
                 RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW | RDW_FRAME);
}

void PluginLoader::handle_editor_drag_start() {
    if (editor_quarantined_.load(std::memory_order_acquire) && !is_gui_thread()) {
        return;
    }
    if (!is_gui_thread()) {
        if (!gui_thread_) {
            return;
        }
        gui_thread().post_async([this]() {
            handle_editor_drag_start();
        });
        return;
    }

    editor_drag_active_ = true;
    editor_drag_started_ms_ = GetTickCount64();
    editor_drag_last_tick_ms_ = editor_drag_started_ms_;
    editor_drag_move_count_ = 0;
    editor_drag_size_count_ = 0;
    editor_drag_windowpos_count_ = 0;
    editor_drag_max_gap_ms_ = 0;
    HWND container_hwnd = reinterpret_cast<HWND>(view_container_hwnd_);
    HWND old_owner = nullptr;
    if (container_hwnd && IsWindow(container_hwnd)) {
        old_owner = reinterpret_cast<HWND>(GetWindowLongPtrW(container_hwnd, GWLP_HWNDPARENT));
        editor_drag_restore_owner_hwnd_ = old_owner;
        if (old_owner) {
            // During the native move/resize modal loop, a cross-process owner
            // relationship can make Windows repeatedly reconcile z-order with
            // the owner viewport. Detach only for the drag; the editor remains
            // a tool window and we restore the owner when the drag ends.
            SetWindowLongPtrW(container_hwnd, GWLP_HWNDPARENT, 0);
            SetWindowPos(container_hwnd,
                         nullptr,
                         0,
                         0,
                         0,
                         0,
                         SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE |
                             SWP_NOOWNERZORDER);
        }
    } else {
        editor_drag_restore_owner_hwnd_ = nullptr;
    }
    blog("editor drag START gui_tid=%lu plugin=\"%s\" hwnd=0x%llx owner=0x%llx tick=%llu",
         GetCurrentThreadId(),
         plugin_name_.empty() ? "(unknown)" : plugin_name_.c_str(),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(container_hwnd)),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(old_owner)),
         static_cast<unsigned long long>(editor_drag_started_ms_));
}

void PluginLoader::handle_editor_drag_tick(uint32_t msg) {
    if (editor_quarantined_.load(std::memory_order_acquire) && !is_gui_thread()) {
        return;
    }
    if (!is_gui_thread()) {
        if (!gui_thread_) {
            return;
        }
        gui_thread().post_async([this, msg]() {
            handle_editor_drag_tick(msg);
        });
        return;
    }

    if (!editor_drag_active_) {
        return;
    }
    const uint64_t now = GetTickCount64();
    if (editor_drag_last_tick_ms_ != 0 && now >= editor_drag_last_tick_ms_) {
        const uint64_t gap = now - editor_drag_last_tick_ms_;
        if (gap > editor_drag_max_gap_ms_) {
            editor_drag_max_gap_ms_ = static_cast<uint32_t>(std::min<uint64_t>(gap, UINT32_MAX));
        }
    }
    editor_drag_last_tick_ms_ = now;
    switch (msg) {
        case WM_MOVE:
        case WM_MOVING:
            ++editor_drag_move_count_;
            break;
        case WM_SIZE:
            ++editor_drag_size_count_;
            break;
        case WM_WINDOWPOSCHANGED:
            ++editor_drag_windowpos_count_;
            break;
        default:
            break;
    }
}

void PluginLoader::handle_editor_drag_end() {
    if (editor_quarantined_.load(std::memory_order_acquire) && !is_gui_thread()) {
        return;
    }
    if (!is_gui_thread()) {
        if (!gui_thread_) {
            return;
        }
        gui_thread().post_async([this]() {
            handle_editor_drag_end();
        });
        return;
    }

    if (!editor_drag_active_) {
        return;
    }
    const uint64_t now = GetTickCount64();
    const uint64_t elapsed = now >= editor_drag_started_ms_ ? now - editor_drag_started_ms_ : 0;
    HWND container_hwnd = reinterpret_cast<HWND>(view_container_hwnd_);
    HWND restore_owner = reinterpret_cast<HWND>(editor_drag_restore_owner_hwnd_);
    if (container_hwnd && IsWindow(container_hwnd) && restore_owner && IsWindow(restore_owner)) {
        SetWindowLongPtrW(container_hwnd, GWLP_HWNDPARENT, reinterpret_cast<LONG_PTR>(restore_owner));
        SetWindowPos(container_hwnd,
                     nullptr,
                     0,
                     0,
                     0,
                     0,
                     SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE |
                         SWP_NOOWNERZORDER);
    }
    blog("editor drag END gui_tid=%lu plugin=\"%s\" hwnd=0x%llx elapsed_ms=%llu move=%u size=%u windowpos=%u max_gap_ms=%u",
         GetCurrentThreadId(),
         plugin_name_.empty() ? "(unknown)" : plugin_name_.c_str(),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(container_hwnd)),
         static_cast<unsigned long long>(elapsed),
         editor_drag_move_count_,
         editor_drag_size_count_,
         editor_drag_windowpos_count_,
         editor_drag_max_gap_ms_);
    editor_drag_active_ = false;
    editor_drag_restore_owner_hwnd_ = nullptr;
}

bool PluginLoader::poll_latency_change(uint32_t& new_latency_out) {
    if (!component_handler_ || !processor_) {
        return false;
    }
    if (!component_handler_->consume_latency_changed_flag()) {
        return false;
    }
    if (!is_gui_thread()) {
        if (!gui_thread_) {
            return false;
        }
        auto result = gui_thread().invoke_sync_for(
            kGuiQueryTimeout,
            "poll_latency_change",
            [this]() -> uint32_t {
                uint32_t latest = static_cast<uint32_t>(processor_->getLatencySamples());
                cached_latency_samples_.store(latest, std::memory_order_release);
                return latest;
            });
        if (!result) {
            return false;
        }
        new_latency_out = *result;
        blog("poll_latency_change: new latency_samples=%u", new_latency_out);
        return true;
    }
    // VST3 規約: kLatencyChanged を受けたら audio 処理を一旦止めて latency を再問い合わせる
    // のが正しい順序。実装簡易のため bridge 側では setActive 再起動はせずに最新値の取得
    // のみ行う。プラグインによっては「内部が再構成されるまで latency が古い値」のことも
    // あるが、次回 polling で正しい値が拾える設計で許容する (= 実害は数十 ms 程度)。
    uint32_t latest = static_cast<uint32_t>(processor_->getLatencySamples());
    cached_latency_samples_.store(latest, std::memory_order_release);
    new_latency_out = latest;
    blog("poll_latency_change: new latency_samples=%u", latest);
    return true;
}

void PluginLoader::notify_host_resize(uint32_t width, uint32_t height) {
    if (editor_quarantined_.load(std::memory_order_acquire) && !is_gui_thread()) {
        return;
    }
    if (!is_gui_thread()) {
        if (!gui_thread_) {
            return;
        }
        gui_thread().post_async([this, width, height]() {
            notify_host_resize(width, height);
        });
        return;
    }

    if (!view_attached_ || !view_) return;
    HWND container_hwnd = reinterpret_cast<HWND>(view_container_hwnd_);
    if (container_hwnd && IsWindow(container_hwnd) && width > 0 && height > 0) {
        resize_frame_for_plugin_client(container_hwnd,
                                       static_cast<int>(width),
                                       static_cast<int>(height));
        layout_editor_child(container_hwnd, reinterpret_cast<HWND>(view_plugin_host_hwnd_));
    }
    const bool size_changed = width != last_gui_width_ || height != last_gui_height_;
    if (!size_changed) {
        return;
    }
    last_gui_width_ = width;
    last_gui_height_ = height;
    Steinberg::ViewRect rect{0, 0,
                             static_cast<Steinberg::int32>(width),
                             static_cast<Steinberg::int32>(height)};
    // タイムスタンプを更新: その後 250ms 間、プラグインから resizeView が
    // 同期 / 非同期 (PostMessage) どちらで来ても SetWindowPos スキップ扱い。
    // Insight2 の「リサイズ中暴れる」フィードバックループ対策。
    if (plug_frame_) {
        plug_frame_->mark_user_resize();
    }
    view_->onSize(&rect);
}

void PluginLoader::hide_gui() {
    if (!is_gui_thread()) {
        if (editor_quarantined_.load(std::memory_order_acquire)) {
            abandon_gui_thread("hide_gui quarantined");
            view_host_hwnd_ = nullptr;
            view_container_hwnd_ = nullptr;
            view_plugin_host_hwnd_ = nullptr;
            view_container_hwnd_snapshot_.store(nullptr, std::memory_order_release);
            gui_surface_visible_ = false;
            gui_app_active_ = true;
            last_gui_width_ = 0;
            last_gui_height_ = 0;
            return;
        }
        if (!gui_thread_) {
            return;
        }
        bool completed = gui_thread_->invoke_void_sync_for(kGuiMutationTimeout, "hide_gui", [this]() {
            hide_gui();
        });
        if (completed) {
            gui_thread_.reset();
        }
        return;
    }

    if (view_attached_ && view_) {
        view_->removed();
        view_->setFrame(nullptr);
    }
    HWND container_hwnd = reinterpret_cast<HWND>(view_container_hwnd_);
    if (container_hwnd && IsWindow(container_hwnd)) {
        DestroyWindow(container_hwnd);
    }
    view_attached_ = false;
    view_host_hwnd_ = nullptr;
    view_container_hwnd_ = nullptr;
    view_plugin_host_hwnd_ = nullptr;
    view_container_hwnd_snapshot_.store(nullptr, std::memory_order_release);
    gui_surface_visible_ = false;
    gui_app_active_ = true;
    last_gui_width_ = 0;
    last_gui_height_ = 0;
    view_ = nullptr;
}

void PluginLoader::reset() {
    if (!processor_) return;
    // VST3 標準: setProcessing(false) → setProcessing(true) でフィルタ履歴 flush
    processor_->setProcessing(false);
    processor_->setProcessing(true);
    // 時刻もリセット (= ProcessContext の sample カウンタ)
    process_time_samples_ = 0;
}

bool PluginLoader::query_state(std::vector<uint8_t>& out_bytes) {
    out_bytes.clear();
    if (!is_gui_thread()) {
        if (!gui_thread_) return false;
        auto result = gui_thread().invoke_sync_for(
            kPluginStateTimeout,
            "query_state",
            [this]() -> std::optional<std::vector<uint8_t>> {
                std::vector<uint8_t> bytes;
                if (!query_state(bytes)) {
                    return std::nullopt;
                }
                return bytes;
            });
        if (!result || !*result) {
            return false;
        }
        out_bytes = std::move(**result);
        return true;
    }
    if (!component_) return false;
    Steinberg::MemoryStream stream;
    if (component_->getState(&stream) != Steinberg::kResultOk) {
        return false;
    }
    Steinberg::TSize size = stream.getSize();
    if (size <= 0) {
        return true;  // 空 state も "成功" (= デフォルト)
    }
    out_bytes.resize(static_cast<size_t>(size));
    Steinberg::int64 zero_pos = 0;
    if (stream.seek(0, Steinberg::IBStream::kIBSeekSet, &zero_pos) != Steinberg::kResultOk) {
        out_bytes.clear();
        return false;
    }
    Steinberg::int32 num_read = 0;
    if (stream.read(out_bytes.data(),
                    static_cast<Steinberg::int32>(size),
                    &num_read) != Steinberg::kResultOk) {
        out_bytes.clear();
        return false;
    }
    out_bytes.resize(static_cast<size_t>(num_read));
    return true;
}

bool PluginLoader::restore_state(const std::vector<uint8_t>& bytes) {
    if (!is_gui_thread()) {
        if (!gui_thread_) return false;
        std::vector<uint8_t> copy = bytes;
        auto result = gui_thread().invoke_sync_for(
            kPluginStateTimeout,
            "restore_state",
            [this, copy = std::move(copy)]() -> bool {
                return restore_state(copy);
            });
        return result.value_or(false);
    }
    if (!component_ || bytes.empty()) return false;
    // MemoryStream にバイト列を書き込む (= 終端後、再度先頭にシークしてから setState)。
    Steinberg::MemoryStream stream;
    Steinberg::int32 written = 0;
    // VST3 SDK の MemoryStream::write は `void*` を取るので const_cast が必要。
    // 中身はコピーされるので呼出側のバイト列は不変。
    if (stream.write(const_cast<uint8_t*>(bytes.data()),
                     static_cast<Steinberg::int32>(bytes.size()),
                     &written) != Steinberg::kResultOk) {
        return false;
    }
    Steinberg::int64 zero_pos = 0;
    if (stream.seek(0, Steinberg::IBStream::kIBSeekSet, &zero_pos) != Steinberg::kResultOk) {
        return false;
    }
    // setState 中の audio 処理クリック対策: 一時的に setProcessing(false) で停止する。
    // VST3 仕様上 setState は active でも許可されているが、内部状態の途中書換による
    // 1 ブロック分のクリックを避けるため。
    // RAII guard で「pause 中に return しても必ず resume される」ことを保証する
    // (= setState 失敗時 / controller setState 失敗時の二重 re-enable 重複を排除)。
    struct ProcessingPauseGuard {
        Steinberg::Vst::IAudioProcessor* p;
        bool was_processing;
        ~ProcessingPauseGuard() {
            if (was_processing && p) p->setProcessing(true);
        }
    };
    bool was_processing = active_;
    if (was_processing && processor_) {
        processor_->setProcessing(false);
    }
    ProcessingPauseGuard guard{processor_.get(), was_processing};
    if (component_->setState(&stream) != Steinberg::kResultOk) {
        return false;
    }
    // controller 側にも同じ state を流して UI 表示と整合させる。
    if (controller_) {
        Steinberg::int64 zero2 = 0;
        if (stream.seek(0, Steinberg::IBStream::kIBSeekSet, &zero2) == Steinberg::kResultOk) {
            controller_->setComponentState(&stream);
        }
    }
    return true;
}

void PluginLoader::flush_with_silence(uint32_t num_samples) {
    if (!processor_ || num_samples == 0) return;
    // process_block の上限 (= setupProcessing 時の maxSamplesPerBlock)。
    // 大きすぎる latency でも安全に分割処理する。
    //
    // **既知の制約** (Codex P2-3, 2026-05-01):
    // この実装は plugin の delay-line を **silence で埋める** だけなので、
    // 純粋 latency plugin (= mIV Test Latency 等) では reset 後 plugin output が
    // 「最初の N samples = silence、その後 = 実 audio」となる。pre-seek tail の
    // 漏れは防げるが、**シーク後 N samples ぶんの silence ギャップ** は残る。
    //
    // 完全な「シーク即時再生」を実現するには post-seek pre-roll discard が必要:
    // 1. reset 後、post-seek 実 audio を N samples 先まで pre-load
    // 2. plugin output (= silence 埋め部分) を **discard** する (= 内部 only)
    // 3. その後の output (= delayed 実 audio) を AudioBuffer に流す
    // これは plugin 内部状態を post-seek に合わせて warm-up することに相当する。
    // 実装には mIV pump 側の協力 (= pre-roll 用 audio 供給 + discard モード) が必要。
    // 現在は silence ギャップを許容して未実装 (= ユーザー報告: 「治った」)。
    // 将来 UX 改善の TODO 候補。
    const uint32_t blk = block_size_ > 0 ? block_size_ : 480;
    const uint32_t channels = 2;
    std::vector<float> silence(blk * channels, 0.0f);
    std::vector<float> dst(blk * channels, 0.0f);
    uint32_t pushed = 0;
    while (pushed < num_samples) {
        uint32_t this_blk = std::min(blk, num_samples - pushed);
        if (!process_block(silence.data(), dst.data(), this_blk)) {
            break;  // process error: 諦めて return
        }
        pushed += this_blk;
    }
}

void PluginLoader::unload() {
    if (!is_gui_thread() && gui_thread_) {
        gui_thread_->invoke_void_sync_for(kPluginStateTimeout, "unload_plugin", [this]() {
            unload();
        });
        gui_thread_.reset();
        return;
    }

    // GUI が出ていれば先に外す (順序逆だとプラグインが crash することがある)
    hide_gui();
    if (active_ && processor_) {
        processor_->setProcessing(false);
    }
    if (active_ && component_) {
        component_->setActive(false);
    }
    active_ = false;
    // ConnectionPoint を切断 (load で connect した分)
    if (component_ && controller_) {
        auto comp_cp = Steinberg::FUnknownPtr<Vst::IConnectionPoint>(component_);
        auto ctrl_cp = Steinberg::FUnknownPtr<Vst::IConnectionPoint>(controller_);
        if (comp_cp && ctrl_cp) {
            comp_cp->disconnect(ctrl_cp);
            ctrl_cp->disconnect(comp_cp);
        }
    }
    if (controller_ && controller_ != Steinberg::FUnknownPtr<Vst::IEditController>(component_)) {
        controller_->terminate();
    }
    controller_ = nullptr;
    if (component_) {
        component_->terminate();
    }
    component_ = nullptr;
    processor_ = nullptr;
    module_.reset();
}

}  // namespace miv
