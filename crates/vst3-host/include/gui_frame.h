#pragma once
#include <cstdint>
#include <functional>
#include <optional>
#include <windows.h>

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
DWORD editor_taskbar_ex_style(DWORD current, bool show_in_taskbar);
enum class EditorSystemCommand { NativeDefault, UserHide, Block, PublishBeforeMinimize };
EditorSystemCommand editor_system_command(bool native_frame, WPARAM command, bool restore_permitted);
std::optional<LRESULT> handle_editor_system_command(bool native_frame, WPARAM command,
    bool restore_permitted, const std::function<void()>& user_hide,
    const std::function<void()>& before_minimize);
int editor_status_height(UINT dpi);
RECT editor_child_bounds(const RECT& client, UINT dpi);
std::optional<SIZE> editor_outer_size(DWORD style, DWORD ex_style, UINT dpi,
                                      int plugin_width, int plugin_height);
bool initialize_native_editor_frame(HWND hwnd, UINT dpi, bool resizable);
std::optional<NormalScreenRect> read_normal_screen_rect(HWND hwnd);
// Runs only on the HWND's thread. No window recreation, ownership change, or
// visibility request is produced. permit is re-read immediately before showing.
bool update_editor_taskbar(HWND hwnd, bool show_in_taskbar, const std::function<bool()>& permit);
bool resize_normal_editor_frame(HWND hwnd, int plugin_width, int plugin_height);
}
