// VST3 プラグインのロード & process loop 実装
//
// Phase 0 POC では「ロードしてパススルーで音を通す」のが目的。
// IComponent / IAudioProcessor / IEditController の取得と最低限の lifecycle 制御まで実装する。

#include "plugin_loader.h"

#include <algorithm>
#include <cwchar>
#include <cstdio>
#include <cstdint>
#include <cstring>
#include <vector>

#include <windows.h>
#include <CommCtrl.h>

#include "pluginterfaces/gui/iplugviewcontentscalesupport.h"

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

constexpr UINT_PTR kPluginChildFocusSubclassId = 0x4D495653534C4D50ull;  // "MIVSSMP"
HWND g_plugin_mouse_hook_host_hwnd = nullptr;
constexpr const wchar_t* kBridgeViewContainerClass = L"MivVst3BridgeViewContainer";

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

LRESULT CALLBACK BridgeViewContainerProc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    if (msg == WM_NCCREATE) {
        auto* cs = reinterpret_cast<CREATESTRUCTW*>(lparam);
        if (cs) {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(cs->lpCreateParams));
        }
    }
    auto* loader = reinterpret_cast<miv::PluginLoader*>(GetWindowLongPtrW(hwnd, GWLP_USERDATA));
    if (msg == WM_CLOSE) {
        if (loader) {
            loader->set_gui_surface_visible_state(false);
        }
        ShowWindow(hwnd, SW_HIDE);
        return 0;
    }
    if (msg == WM_SYSCOMMAND && ((wparam & 0xFFF0) == SC_CLOSE)) {
        if (loader) {
            loader->set_gui_surface_visible_state(false);
        }
        ShowWindow(hwnd, SW_HIDE);
        return 0;
    }
    if (msg == WM_SIZE && wparam != SIZE_MINIMIZED) {
        if (loader) {
            loader->handle_editor_window_size();
        }
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
        blog("bridge container WM_ACTIVATEAPP active=%d", wparam != FALSE ? 1 : 0);
        // Do not lower the surface directly here. Moving focus between the Rust
        // host HWND and the bridge-owned container can deactivate this process
        // even though the mIV window group is still the foreground UI. The Rust
        // side polls the actual foreground process and sends set_gui_app_active
        // when the group really leaves or re-enters the foreground.
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

HWND create_bridge_view_container(const miv::GuiWindowOptions& options, miv::PluginLoader* loader) {
    HWND owner_hwnd = reinterpret_cast<HWND>(options.owner_hwnd);
    if (!ensure_bridge_view_container_class()) {
        return nullptr;
    }
    const DWORD style = WS_POPUP | WS_CAPTION |
                        (options.resizable ? WS_THICKFRAME : 0) |
                        WS_CLIPCHILDREN | WS_CLIPSIBLINGS;
    const DWORD ex_style = WS_EX_TOOLWINDOW | WS_EX_WINDOWEDGE;
    UINT dpi = owner_hwnd ? GetDpiForWindow(owner_hwnd) : GetDpiForSystem();
    if (dpi == 0) dpi = 96;
    RECT outer{0,
               0,
               static_cast<LONG>(std::max<uint32_t>(1, options.width)),
               static_cast<LONG>(std::max<uint32_t>(1, options.height))};
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
        return nullptr;
    }
    blog("bridge editor window created hwnd=0x%llx owner=0x%llx pos=%d,%d client=%ux%u outer=%dx%d",
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(container)),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(owner_hwnd)),
         x,
         y,
         options.width,
         options.height,
         width,
         height);
    return container;
}

bool is_context_menu_message(UINT msg) {
    return msg == WM_RBUTTONDOWN || msg == WM_RBUTTONUP ||
           msg == WM_NCRBUTTONDOWN || msg == WM_NCRBUTTONUP ||
           msg == WM_CONTEXTMENU;
}

bool should_prepare_context_menu_focus(UINT msg) {
    return msg == WM_RBUTTONDOWN || msg == WM_NCRBUTTONDOWN ||
           msg == WM_CONTEXTMENU;
}

bool is_context_diagnostic_message(UINT msg) {
    return is_context_menu_message(msg) || msg == WM_CANCELMODE ||
           msg == WM_CAPTURECHANGED || msg == WM_KILLFOCUS ||
           msg == WM_SETFOCUS || msg == WM_MOUSEACTIVATE;
}

const char* window_message_name(UINT msg) {
    switch (msg) {
    case WM_RBUTTONDOWN:
        return "WM_RBUTTONDOWN";
    case WM_RBUTTONUP:
        return "WM_RBUTTONUP";
    case WM_NCRBUTTONDOWN:
        return "WM_NCRBUTTONDOWN";
    case WM_NCRBUTTONUP:
        return "WM_NCRBUTTONUP";
    case WM_CONTEXTMENU:
        return "WM_CONTEXTMENU";
    case WM_CANCELMODE:
        return "WM_CANCELMODE";
    case WM_CAPTURECHANGED:
        return "WM_CAPTURECHANGED";
    case WM_KILLFOCUS:
        return "WM_KILLFOCUS";
    case WM_SETFOCUS:
        return "WM_SETFOCUS";
    case WM_MOUSEACTIVATE:
        return "WM_MOUSEACTIVATE";
    default:
        return "WM_*";
    }
}

const char* win_event_name(DWORD event) {
    switch (event) {
    case EVENT_OBJECT_CREATE:
        return "create";
    case EVENT_OBJECT_SHOW:
        return "show";
    case EVENT_OBJECT_HIDE:
        return "hide";
    case EVENT_OBJECT_DESTROY:
        return "destroy";
    default:
        return "event";
    }
}

void normalize_plugin_top_level_window(HWND hwnd) {
    HWND owner_hwnd = g_plugin_mouse_hook_host_hwnd;
    if (!hwnd || !IsWindow(hwnd) || !owner_hwnd || !IsWindow(owner_hwnd) || hwnd == owner_hwnd) {
        return;
    }

    DWORD pid = 0;
    GetWindowThreadProcessId(hwnd, &pid);
    if (pid != GetCurrentProcessId()) {
        return;
    }

    LONG_PTR style = GetWindowLongPtrW(hwnd, GWL_STYLE);
    if ((style & WS_CHILD) != 0) {
        return;
    }
    HWND parent = GetParent(hwnd);
    if (parent != nullptr) {
        return;
    }

    wchar_t class_name[128] = {};
    GetClassNameW(hwnd, class_name, 128);
    if (std::wcscmp(class_name, L"#32768") == 0) {
        return;  // system menu window
    }

    LONG_PTR ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    HWND current_owner = GetWindow(hwnd, GW_OWNER);
    const bool needs_toolwindow = (ex_style & WS_EX_TOOLWINDOW) == 0 ||
                                  (ex_style & WS_EX_APPWINDOW) != 0;
    const bool needs_owner = current_owner == nullptr;
    if (!needs_toolwindow && !needs_owner) {
        return;
    }

    if (needs_owner) {
        SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, reinterpret_cast<LONG_PTR>(owner_hwnd));
    }
    if (needs_toolwindow) {
        LONG_PTR next_ex_style = (ex_style | WS_EX_TOOLWINDOW) & ~WS_EX_APPWINDOW;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next_ex_style);
    }
    SetWindowPos(hwnd,
                 nullptr,
                 0,
                 0,
                 0,
                 0,
                 SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE |
                     SWP_FRAMECHANGED);
    blog("normalized plugin top-level hwnd=0x%llx owner=0x%llx",
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(hwnd)),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(owner_hwnd)));
}

BOOL CALLBACK EnumPluginTopLevelNormalizeProc(HWND hwnd, LPARAM) {
    normalize_plugin_top_level_window(hwnd);
    return TRUE;
}

void log_window_event(DWORD event, HWND hwnd, LONG object_id, LONG child_id) {
    if (!hwnd || object_id != OBJID_WINDOW || child_id != CHILDID_SELF || !IsWindow(hwnd)) {
        return;
    }

    DWORD pid = 0;
    DWORD thread_id = GetWindowThreadProcessId(hwnd, &pid);
    if (pid != GetCurrentProcessId()) {
        return;
    }

    wchar_t class_name[128] = {};
    GetClassNameW(hwnd, class_name, 128);
    wchar_t title[128] = {};
    GetWindowTextW(hwnd, title, 128);
    LONG_PTR style = GetWindowLongPtrW(hwnd, GWL_STYLE);
    LONG_PTR ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    HWND owner = GetWindow(hwnd, GW_OWNER);
    HWND parent = GetParent(hwnd);

    char class_utf8[256] = {};
    WideCharToMultiByte(CP_UTF8, 0, class_name, -1, class_utf8, sizeof(class_utf8), nullptr, nullptr);
    char title_utf8[256] = {};
    WideCharToMultiByte(CP_UTF8, 0, title, -1, title_utf8, sizeof(title_utf8), nullptr, nullptr);

    const bool is_popup = (style & WS_POPUP) != 0;
    const bool is_toolwindow = (ex_style & WS_EX_TOOLWINDOW) != 0;
    const bool is_menuish = std::strstr(class_utf8, "Menu") != nullptr ||
                            std::strstr(class_utf8, "Popup") != nullptr ||
                            std::strstr(class_utf8, "SSL") != nullptr ||
                            is_popup || is_toolwindow;

    blog("winevent %s hwnd=0x%llx class=\"%s\" title=\"%s\" thread=%lu owner=0x%llx parent=0x%llx style=0x%llx ex=0x%llx menuish=%d fg=0x%llx",
         win_event_name(event),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(hwnd)),
         class_utf8,
         title_utf8,
         static_cast<unsigned long>(thread_id),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(owner)),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(parent)),
         static_cast<unsigned long long>(style),
         static_cast<unsigned long long>(ex_style),
         is_menuish ? 1 : 0,
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(GetForegroundWindow())));
}

void CALLBACK PluginPopupWinEventProc(HWINEVENTHOOK,
                                      DWORD event,
                                      HWND hwnd,
                                      LONG object_id,
                                      LONG child_id,
                                      DWORD,
                                      DWORD) {
    if ((event == EVENT_OBJECT_CREATE || event == EVENT_OBJECT_SHOW) &&
        object_id == OBJID_WINDOW && child_id == CHILDID_SELF) {
        normalize_plugin_top_level_window(hwnd);
    }
    log_window_event(event, hwnd, object_id, child_id);
}

void prepare_context_menu_focus(HWND host_hwnd, HWND focus_hwnd, const char* source, UINT msg) {
    if (!host_hwnd || !IsWindow(host_hwnd)) {
        return;
    }

    HWND root_hwnd = GetAncestor(host_hwnd, GA_ROOT);
    if (!root_hwnd || !IsWindow(root_hwnd)) {
        root_hwnd = host_hwnd;
    }

    HWND foreground_before = GetForegroundWindow();
    HWND active_before = GetActiveWindow();
    HWND focus_before = GetFocus();

    DWORD root_pid = 0;
    const DWORD root_thread = GetWindowThreadProcessId(root_hwnd, &root_pid);
    DWORD focus_pid = 0;
    const DWORD focus_thread =
        focus_hwnd && IsWindow(focus_hwnd) ? GetWindowThreadProcessId(focus_hwnd, &focus_pid) : 0;
    DWORD foreground_pid = 0;
    const DWORD foreground_thread = foreground_before
        ? GetWindowThreadProcessId(foreground_before, &foreground_pid)
        : 0;
    const DWORD current_thread = GetCurrentThreadId();

    blog("right-click %s: msg=0x%X focus=0x%llx host=0x%llx root=0x%llx fg=0x%llx active=0x%llx prev_focus=0x%llx threads cur=%lu root=%lu focus=%lu fg=%lu",
         source,
         static_cast<unsigned int>(msg),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(focus_hwnd)),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(host_hwnd)),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(root_hwnd)),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(foreground_before)),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(active_before)),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(focus_before)),
         static_cast<unsigned long>(current_thread),
         static_cast<unsigned long>(root_thread),
         static_cast<unsigned long>(focus_thread),
         static_cast<unsigned long>(foreground_thread));

    const bool attach_root = root_thread != 0 && root_thread != current_thread;
    const bool attach_focus = focus_thread != 0 && focus_thread != current_thread &&
                              focus_thread != root_thread;
    const bool attach_foreground = foreground_thread != 0 && foreground_thread != current_thread &&
                                   foreground_thread != root_thread &&
                                   foreground_thread != focus_thread;

    if (attach_root) {
        AttachThreadInput(current_thread, root_thread, TRUE);
    }
    if (attach_focus) {
        AttachThreadInput(current_thread, focus_thread, TRUE);
    }
    if (attach_foreground) {
        AttachThreadInput(current_thread, foreground_thread, TRUE);
    }

    SetForegroundWindow(root_hwnd);
    BringWindowToTop(root_hwnd);
    SetActiveWindow(root_hwnd);

    if (focus_hwnd && IsWindow(focus_hwnd)) {
        SetFocus(focus_hwnd);
    }

    HWND foreground_after = GetForegroundWindow();
    HWND active_after = GetActiveWindow();
    HWND focus_after = GetFocus();
    blog("right-click %s after: fg=0x%llx active=0x%llx focus=0x%llx",
         source,
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(foreground_after)),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(active_after)),
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(focus_after)));

    if (attach_foreground) {
        AttachThreadInput(current_thread, foreground_thread, FALSE);
    }
    if (attach_focus) {
        AttachThreadInput(current_thread, focus_thread, FALSE);
    }
    if (attach_root) {
        AttachThreadInput(current_thread, root_thread, FALSE);
    }
}

LRESULT CALLBACK PluginChildFocusSubclassProc(HWND hwnd,
                                              UINT msg,
                                              WPARAM wparam,
                                              LPARAM lparam,
                                              UINT_PTR subclass_id,
                                              DWORD_PTR ref_data) {
    HWND host_hwnd = reinterpret_cast<HWND>(ref_data);
    if (is_context_diagnostic_message(msg)) {
        blog("plugin child msg: %s(0x%X) hwnd=0x%llx host=0x%llx fg=0x%llx active=0x%llx focus=0x%llx capture=0x%llx w=0x%llx l=0x%llx",
             window_message_name(msg),
             static_cast<unsigned int>(msg),
             static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(hwnd)),
             static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(host_hwnd)),
             static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(GetForegroundWindow())),
             static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(GetActiveWindow())),
             static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(GetFocus())),
             static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(GetCapture())),
             static_cast<unsigned long long>(wparam),
             static_cast<unsigned long long>(lparam));
    }
    if (should_prepare_context_menu_focus(msg) && host_hwnd && IsWindow(host_hwnd)) {
        // SSL Meter Pro opens its popup menu from the plugin child HWND. The
        // popup is stable only when the top-level owner is foreground before
        // the plugin's own WndProc reaches TrackPopupMenu. Its graphical menu
        // also appears to require focus to remain on the plugin child itself
        // (Pro-Q's simpler text menu works without this extra focus nudge).
        prepare_context_menu_focus(host_hwnd, hwnd, "subclass", msg);
    }
    if (msg == WM_NCDESTROY) {
        RemoveWindowSubclass(hwnd, PluginChildFocusSubclassProc, subclass_id);
    }
    return DefSubclassProc(hwnd, msg, wparam, lparam);
}

LRESULT CALLBACK PluginMouseHookProc(int code, WPARAM wparam, LPARAM lparam) {
    if (code >= 0 && is_context_menu_message(static_cast<UINT>(wparam))) {
        auto* mouse = reinterpret_cast<MOUSEHOOKSTRUCT*>(lparam);
        HWND target_hwnd = mouse ? mouse->hwnd : nullptr;
        HWND host_hwnd = g_plugin_mouse_hook_host_hwnd;
        if (host_hwnd && IsWindow(host_hwnd)) {
            DWORD_PTR existing_ref_data = 0;
            const bool covered_by_subclass =
                target_hwnd && IsWindow(target_hwnd) &&
                GetWindowSubclass(target_hwnd,
                                  PluginChildFocusSubclassProc,
                                  kPluginChildFocusSubclassId,
                                  &existing_ref_data) != FALSE;
            if (covered_by_subclass) {
                blog("right-click mouse hook covered-by-subclass: %s(0x%X) focus=0x%llx host=0x%llx",
                     window_message_name(static_cast<UINT>(wparam)),
                     static_cast<unsigned int>(wparam),
                     static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(target_hwnd)),
                     static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(host_hwnd)));
            } else if (should_prepare_context_menu_focus(static_cast<UINT>(wparam))) {
                prepare_context_menu_focus(host_hwnd, target_hwnd, "mouse hook", static_cast<UINT>(wparam));
            } else {
                blog("right-click mouse hook observed: %s(0x%X) focus=0x%llx host=0x%llx",
                     window_message_name(static_cast<UINT>(wparam)),
                     static_cast<unsigned int>(wparam),
                     static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(target_hwnd)),
                     static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(host_hwnd)));
            }
        }
    }
    return CallNextHookEx(nullptr, code, wparam, lparam);
}

struct ChildFocusHookContext {
    HWND host_hwnd = nullptr;
    std::vector<void*>* tracked = nullptr;
    std::vector<miv::PluginMouseHookEntry>* mouse_hooks = nullptr;
};

BOOL CALLBACK EnumChildFocusHookProc(HWND child_hwnd, LPARAM lparam) {
    auto* ctx = reinterpret_cast<ChildFocusHookContext*>(lparam);
    if (!ctx || !ctx->tracked || !child_hwnd || !IsWindow(child_hwnd)) {
        return TRUE;
    }

    DWORD pid = 0;
    DWORD thread_id = GetWindowThreadProcessId(child_hwnd, &pid);
    if (pid != GetCurrentProcessId()) {
        return TRUE;
    }

    if (ctx->mouse_hooks && thread_id != 0) {
        const bool already_hooked =
            std::any_of(ctx->mouse_hooks->begin(),
                        ctx->mouse_hooks->end(),
                        [thread_id](const miv::PluginMouseHookEntry& entry) {
                            return entry.thread_id == thread_id && entry.hook != nullptr;
                        });
        if (!already_hooked) {
            HHOOK hook = SetWindowsHookExW(WH_MOUSE, PluginMouseHookProc, nullptr, thread_id);
            if (hook) {
                ctx->mouse_hooks->push_back(
                    miv::PluginMouseHookEntry{thread_id, reinterpret_cast<void*>(hook)});
                blog("child focus mouse hook installed thread=%lu",
                     static_cast<unsigned long>(thread_id));
            } else {
                blog("child focus mouse hook install failed thread=%lu err=%lu",
                     static_cast<unsigned long>(thread_id),
                     static_cast<unsigned long>(GetLastError()));
            }
        }
    }

    DWORD_PTR existing_ref_data = 0;
    const bool already_subclassed =
        GetWindowSubclass(child_hwnd,
                          PluginChildFocusSubclassProc,
                          kPluginChildFocusSubclassId,
                          &existing_ref_data) != FALSE;
    void* raw = reinterpret_cast<void*>(child_hwnd);
    const bool already_tracked =
        std::find(ctx->tracked->begin(), ctx->tracked->end(), raw) != ctx->tracked->end();
    if (already_subclassed) {
        if (!already_tracked) {
            ctx->tracked->push_back(raw);
        }
        return TRUE;
    }

    if (SetWindowSubclass(child_hwnd,
                          PluginChildFocusSubclassProc,
                          kPluginChildFocusSubclassId,
                          reinterpret_cast<DWORD_PTR>(ctx->host_hwnd))) {
        if (!already_tracked) {
            ctx->tracked->push_back(raw);
        }
        blog("child focus hook installed hwnd=0x%llx",
             static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(child_hwnd)));
    } else {
        blog("child focus hook install failed hwnd=0x%llx err=%lu",
             static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(child_hwnd)),
             static_cast<unsigned long>(GetLastError()));
    }
    return TRUE;
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

PluginLoader::PluginLoader() {
    host_app_ = owned(new HostApplication);
    component_handler_ = owned(new ComponentHandler);
    plug_frame_ = owned(new PlugFrame);
}

PluginLoader::~PluginLoader() {
    unload();
}

bool PluginLoader::probe(const std::string& plugin_path,
                         PluginProbeInfo& info_out,
                         std::string& error_out) {
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
    sample_rate_ = sample_rate;
    block_size_ = block_size;

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

    cached_latency_samples_ = static_cast<uint32_t>(processor_->getLatencySamples());
    info_out.latency_samples = cached_latency_samples_;

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

void PluginLoader::install_child_focus_hooks(void* host_hwnd_raw) {
    HWND host_hwnd = reinterpret_cast<HWND>(host_hwnd_raw);
    if (!host_hwnd || !IsWindow(host_hwnd)) {
        return;
    }

    // Drop stale HWNDs left by plugin-side child recreation. Live hooks remain
    // installed; repeated enumeration is cheap and lets resize-created children
    // pick up the same foreground fix without a global WinEvent hook.
    child_focus_hook_hwnds_.erase(
        std::remove_if(child_focus_hook_hwnds_.begin(),
                       child_focus_hook_hwnds_.end(),
                       [](void* raw) {
                           HWND hwnd = reinterpret_cast<HWND>(raw);
                           return !hwnd || !IsWindow(hwnd);
                       }),
        child_focus_hook_hwnds_.end());

    HWND focus_host = reinterpret_cast<HWND>(view_container_hwnd_);
    if (!focus_host || !IsWindow(focus_host)) {
        focus_host = host_hwnd;
    }
    const auto before = child_focus_hook_hwnds_.size();
    g_plugin_mouse_hook_host_hwnd = focus_host;
    if (!popup_event_hook_) {
        HWINEVENTHOOK hook = SetWinEventHook(EVENT_OBJECT_CREATE,
                                             EVENT_OBJECT_HIDE,
                                             nullptr,
                                             PluginPopupWinEventProc,
                                             GetCurrentProcessId(),
                                             0,
                                             WINEVENT_OUTOFCONTEXT);
        if (hook) {
            popup_event_hook_ = reinterpret_cast<void*>(hook);
            blog("popup WinEvent hook installed");
        } else {
            blog("popup WinEvent hook install failed err=%lu",
                 static_cast<unsigned long>(GetLastError()));
        }
    }
    HWND enum_root = reinterpret_cast<HWND>(view_container_hwnd_);
    if (!enum_root || !IsWindow(enum_root)) {
        enum_root = host_hwnd;
    }
    EnumWindows(EnumPluginTopLevelNormalizeProc, 0);
    ChildFocusHookContext ctx{focus_host, &child_focus_hook_hwnds_, &child_focus_mouse_hooks_};
    EnumChildFocusHookProc(enum_root, reinterpret_cast<LPARAM>(&ctx));
    EnumChildWindows(enum_root, EnumChildFocusHookProc, reinterpret_cast<LPARAM>(&ctx));
    const auto installed = child_focus_hook_hwnds_.size() - before;
    if (installed > 0) {
        blog("install_child_focus_hooks: installed=%zu total=%zu",
             installed,
             child_focus_hook_hwnds_.size());
    }
}

void PluginLoader::remove_child_focus_hooks() {
    if (popup_event_hook_) {
        UnhookWinEvent(reinterpret_cast<HWINEVENTHOOK>(popup_event_hook_));
        popup_event_hook_ = nullptr;
    }
    for (const auto& entry : child_focus_mouse_hooks_) {
        HHOOK hook = reinterpret_cast<HHOOK>(entry.hook);
        if (hook) {
            UnhookWindowsHookEx(hook);
        }
    }
    child_focus_mouse_hooks_.clear();
    g_plugin_mouse_hook_host_hwnd = nullptr;

    for (void* raw : child_focus_hook_hwnds_) {
        HWND hwnd = reinterpret_cast<HWND>(raw);
        if (hwnd && IsWindow(hwnd)) {
            RemoveWindowSubclass(hwnd, PluginChildFocusSubclassProc, kPluginChildFocusSubclassId);
        }
    }
    child_focus_hook_hwnds_.clear();
}

bool PluginLoader::show_gui(const GuiWindowOptions& options, bool visible, std::string& error_out) {
    blog("show_gui start owner=0x%llx visible=%d",
         (unsigned long long)options.owner_hwnd,
         visible ? 1 : 0);
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

    HWND attach_hwnd = create_bridge_view_container(options, this);
    if (!attach_hwnd) {
        error_out = "failed to create editor window";
        view_->setFrame(nullptr);
        view_ = nullptr;
        return false;
    }

    plug_frame_->set_host_hwnd(attach_hwnd);
    blog("show_gui: attached(hwnd, HWND) editor=0x%llx",
         static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(attach_hwnd)));
    if (view_->attached(attach_hwnd, Steinberg::kPlatformTypeHWND) != Steinberg::kResultOk) {
        error_out = "attached() failed";
        if (IsWindow(attach_hwnd)) {
            DestroyWindow(attach_hwnd);
        }
        view_->setFrame(nullptr);
        view_ = nullptr;
        return false;
    }
    view_attached_ = true;
    view_host_hwnd_ = options.owner_hwnd;
    view_container_hwnd_ = attach_hwnd;
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
            RECT outer{0, 0, preferred_w, preferred_h};
            DWORD style = static_cast<DWORD>(GetWindowLongPtrW(container_hwnd, GWL_STYLE));
            DWORD ex_style = static_cast<DWORD>(GetWindowLongPtrW(container_hwnd, GWL_EXSTYLE));
            UINT dpi = GetDpiForWindow(container_hwnd);
            if (dpi == 0) dpi = 96;
            AdjustWindowRectExForDpi(&outer, style, FALSE, ex_style, dpi);
            SetWindowPos(container_hwnd,
                         nullptr,
                         0,
                         0,
                         outer.right - outer.left,
                         outer.bottom - outer.top,
                         SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
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
    install_child_focus_hooks(attach_hwnd);
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
    if (plug_frame_) {
        plug_frame_->set_user_resizing(active);
    }
}

void PluginLoader::refresh_gui_surface(void* container_hwnd_ptr) {
    HWND container_hwnd = reinterpret_cast<HWND>(container_hwnd_ptr);
    if (!container_hwnd || !IsWindow(container_hwnd)) {
        return;
    }
    if (view_attached_ && view_) {
        RECT client{};
        if (GetClientRect(container_hwnd, &client)) {
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
    gui_surface_visible_ = visible;
}

bool PluginLoader::gui_surface_should_show() const {
    return gui_surface_visible_ && gui_app_active_;
}

bool PluginLoader::gui_surface_target_rect(int32_t& x_out,
                                           int32_t& y_out,
                                           int32_t& width_out,
                                           int32_t& height_out) {
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
    refresh_gui_surface(view_container_hwnd_);
}

void PluginLoader::set_gui_visible(bool visible) {
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
    HWND container_hwnd = reinterpret_cast<HWND>(view_container_hwnd_);
    if (container_hwnd && IsWindow(container_hwnd)) {
        SetWindowPos(container_hwnd,
                     topmost ? HWND_TOPMOST : HWND_NOTOPMOST,
                     0,
                     0,
                     0,
                     0,
                     SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        blog("set_gui_topmost: bridge surface hwnd=0x%llx topmost=%d",
             static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(container_hwnd)),
             topmost ? 1 : 0);
        return;
    }
    blog("set_gui_topmost: no bridge surface topmost=%d", topmost ? 1 : 0);
}

void PluginLoader::set_gui_app_active(bool active) {
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
                         SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
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
    HWND container_hwnd = reinterpret_cast<HWND>(view_container_hwnd_);
    if (!view_attached_ || !view_ || !container_hwnd || !IsWindow(container_hwnd)) {
        return;
    }
    RECT client{};
    if (!GetClientRect(container_hwnd, &client)) {
        return;
    }
    const uint32_t width = static_cast<uint32_t>(std::max<LONG>(1, client.right - client.left));
    const uint32_t height = static_cast<uint32_t>(std::max<LONG>(1, client.bottom - client.top));
    if (width == last_gui_width_ && height == last_gui_height_) {
        return;
    }
    last_gui_width_ = width;
    last_gui_height_ = height;
    if (plug_frame_) {
        plug_frame_->mark_user_resize();
    }
    Steinberg::ViewRect rect{0,
                             0,
                             static_cast<Steinberg::int32>(width),
                             static_cast<Steinberg::int32>(height)};
    view_->onSize(&rect);
}

bool PluginLoader::poll_latency_change(uint32_t& new_latency_out) {
    if (!component_handler_ || !processor_) {
        return false;
    }
    if (!component_handler_->consume_latency_changed_flag()) {
        return false;
    }
    // VST3 規約: kLatencyChanged を受けたら audio 処理を一旦止めて latency を再問い合わせる
    // のが正しい順序。実装簡易のため bridge 側では setActive 再起動はせずに最新値の取得
    // のみ行う。プラグインによっては「内部が再構成されるまで latency が古い値」のことも
    // あるが、次回 polling で正しい値が拾える設計で許容する (= 実害は数十 ms 程度)。
    uint32_t latest = static_cast<uint32_t>(processor_->getLatencySamples());
    cached_latency_samples_ = latest;
    new_latency_out = latest;
    blog("poll_latency_change: new latency_samples=%u", latest);
    return true;
}

void PluginLoader::notify_host_resize(uint32_t width, uint32_t height) {
    if (!view_attached_ || !view_) return;
    HWND container_hwnd = reinterpret_cast<HWND>(view_container_hwnd_);
    if (container_hwnd && IsWindow(container_hwnd) && width > 0 && height > 0) {
        RECT outer{0, 0, static_cast<LONG>(width), static_cast<LONG>(height)};
        DWORD style = static_cast<DWORD>(GetWindowLongPtrW(container_hwnd, GWL_STYLE));
        DWORD ex_style = static_cast<DWORD>(GetWindowLongPtrW(container_hwnd, GWL_EXSTYLE));
        UINT dpi = GetDpiForWindow(container_hwnd);
        if (dpi == 0) dpi = 96;
        AdjustWindowRectExForDpi(&outer, style, FALSE, ex_style, dpi);
        SetWindowPos(container_hwnd,
                     nullptr,
                     0,
                     0,
                     outer.right - outer.left,
                     outer.bottom - outer.top,
                     SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
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
    install_child_focus_hooks(view_host_hwnd_);
}

void PluginLoader::hide_gui() {
    remove_child_focus_hooks();
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
