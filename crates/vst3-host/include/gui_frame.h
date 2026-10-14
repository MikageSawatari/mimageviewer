#pragma once
#include <cstdint>
#include <functional>
#include <optional>
#include <windows.h>

struct ITaskbarList;

namespace miv {
// Canonical virtual-screen physical pixels, never an iconic/window-workspace rect.
struct NormalScreenRect {
    int32_t x = 0, y = 0;
    uint32_t outer_width = 0, outer_height = 0;
};
std::optional<NormalScreenRect> placement_to_screen(const RECT& normal, DWORD ex_style,
                                                   const RECT& monitor, const RECT& work);
std::optional<RECT> screen_to_placement(const NormalScreenRect& normal, DWORD ex_style,
                                       const RECT& monitor, const RECT& work);
DWORD editor_frame_style(bool native_caption, bool resizable);
bool editor_uses_saved_outer_size(bool native_caption, bool resizable, bool has_saved_size);
// EffeTune always has a normal caption; Shell membership never changes styles.
DWORD editor_ex_style(DWORD current, bool native_caption);
bool editor_taskbar_requires_user_hide(bool show_in_taskbar, bool iconic);
enum class EditorSystemCommand { NativeDefault, UserHide, Block, PublishBeforeMinimize };
EditorSystemCommand editor_system_command(bool native_frame, WPARAM command, bool restore_permitted,
                                           bool show_in_taskbar = true);
std::optional<LRESULT> handle_editor_system_command(bool native_frame, WPARAM command,
    bool restore_permitted, const std::function<void()>& user_hide,
    const std::function<void()>& before_minimize, bool show_in_taskbar = true);
int editor_status_height(UINT dpi);
RECT editor_child_bounds(const RECT& client, UINT dpi);
// Iconic HWND clients are not editor allocations. Hidden normal windows remain
// eligible for attach and ordinary resize acknowledgements.
std::optional<RECT> read_normal_editor_client(HWND frame);
std::optional<SIZE> layout_normal_editor_child(HWND frame, HWND child);
bool notify_normal_editor_view(HWND frame, const std::function<void()>& notify);
bool apply_normal_editor_client(HWND frame, HWND child, uint32_t& last_width,
    uint32_t& last_height, bool force_notify, const std::function<void(SIZE)>& notify);
std::optional<SIZE> editor_outer_size(DWORD style, DWORD ex_style, UINT dpi,
                                      int plugin_width, int plugin_height);
bool initialize_native_editor_frame(HWND hwnd, UINT dpi, bool resizable);
std::optional<NormalScreenRect> read_normal_screen_rect(HWND hwnd);
UINT editor_taskbar_button_created_message();
UINT editor_taskbar_created_message();
// Created, used and destroyed on the HWND's COM-initialized GUI thread. The
// injected sink lets inert-HWND tests exercise lifecycle without Explorer.
class EditorTaskbar {
public:
    using Sink = std::function<HRESULT(HWND, bool add)>;
    EditorTaskbar(HWND hwnd, bool enabled, Sink sink = {});
    ~EditorTaskbar();
    EditorTaskbar(const EditorTaskbar&) = delete;
    EditorTaskbar& operator=(const EditorTaskbar&) = delete;
    bool enabled() const { return enabled_; }
    void set_enabled(bool enabled) { enabled_ = enabled; }
    HRESULT sync(bool visible, bool permitted);
    HRESULT detach();
    HRESULT shell_recreated();
private:
    HRESULT initialize();
    HRESULT apply(bool add);
    HWND hwnd_;
    HWND added_ = nullptr;
    bool enabled_;
    Sink sink_;
    ITaskbarList* shell_ = nullptr;
    HRESULT initialization_ = E_UNEXPECTED;
};
bool resize_normal_editor_frame(HWND hwnd, int plugin_width, int plugin_height);
}
