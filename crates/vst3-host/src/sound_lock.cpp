#include "sound_lock.h"
#include "gui_frame.h"
#include <algorithm>
#include <cstdio>
#include <utility>

namespace miv {
void initialize_sound_owner(std::string_view plugin_name, const std::wstring& data_dir,
                            const std::function<void(const std::wstring&)>& create) {
    if (plugin_name == "mIV Sound" && !data_dir.empty()) create(data_dir);
}
std::wstring SoundPresetFile::environment_directory() {
    const DWORD size = GetEnvironmentVariableW(L"MIV_SOUND_DATA_DIR", nullptr, 0);
    if (!size) return {};
    std::wstring value(size, L'\0');
    const DWORD copied = GetEnvironmentVariableW(L"MIV_SOUND_DATA_DIR", value.data(), size);
    if (!copied || copied >= size) return {};
    value.resize(copied);
    return value;
}
SoundPresetFile::SoundPresetFile(std::wstring dir) {
    if (!dir.empty()) {
        path_ = dir + L"\\plugin\\effetune_presets.json";
        temporary_ = path_ + L".miv-mcp.tmp";
    }
}
SoundPresetFile::~SoundPresetFile() { release(); }
SoundGuard SoundPresetFile::acquire() {
    release();
    if (path_.empty()) return guard_ = {SoundGuard::Kind::Unavailable, 0};
    handle_ = CreateFileW(path_.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr,
                          OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (handle_ != INVALID_HANDLE_VALUE)
        return guard_ = {SoundGuard::Kind::Held, ++epoch_};
    const auto error = GetLastError();
    return guard_ = {(error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND)
        ? SoundGuard::Kind::Absent : SoundGuard::Kind::Unavailable, 0};
}
bool SoundPresetFile::matches(SoundGuard expected) const {
    if (expected.kind != guard_.kind) return false;
    if (expected.kind == SoundGuard::Kind::Held)
        return handle_ != INVALID_HANDLE_VALUE && expected.epoch == guard_.epoch;
    if (expected.kind != SoundGuard::Kind::Absent || path_.empty()) return false;
    if (GetFileAttributesW(path_.c_str()) != INVALID_FILE_ATTRIBUTES) return false;
    const auto error = GetLastError();
    return error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND;
}
bool SoundPresetFile::commit() {
    release();
    const bool ok = MoveFileExW(temporary_.c_str(), path_.c_str(),
                               MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) != FALSE;
    const auto error = ok ? ERROR_SUCCESS : GetLastError();
    acquire();
    if (guard_.kind == SoundGuard::Kind::Unavailable)
        std::fprintf(stderr, "[BRIDGE] sound preset guard reopen failed err=%lu\n", GetLastError());
    if (!ok) std::fprintf(stderr, "[BRIDGE] sound preset commit failed err=%lu\n", error);
    return ok;
}
void SoundPresetFile::release() {
    if (handle_ != INVALID_HANDLE_VALUE) CloseHandle(handle_);
    handle_ = INVALID_HANDLE_VALUE;
    guard_ = {};
}
SoundLock::SoundLock(SoundPlugin& plugin, SoundPresetBoundary& presets,
                     std::function<std::string()> token)
    : plugin_(plugin), presets_(presets), token_(std::move(token)) {}
SoundReply SoundLock::reply(const char* status) const {
    SoundReply out;
    out.status = status;
    if (session_) {
        out.lock = session_->token;
        out.generation = session_->generation;
        out.guard = session_->guard;
    }
    return out;
}
std::optional<std::string> SoundLock::release() {
    if (!session_) return {};
    auto token = session_->token;
    session_.reset();
    presets_.release();
    plugin_.show_lock(false);
    return token;
}
SoundReply SoundLock::execute(const SoundRequest& req) {
    if (!plugin_.eligible()) return reply("Rejected");
    if (req.command == "sound_lock") {
        if (session_) return reply("Busy");
        auto guard = presets_.acquire();
        if (guard.kind == SoundGuard::Kind::Unavailable) return reply("Rejected");
        session_ = Session{token_(), 0, guard, {}};
        plugin_.show_lock(true);
        return reply("Locked");
    }
    if (req.command == "sound_get") {
        auto out = reply("State");
        SoundBytes bytes;
        if (!plugin_.get(bytes)) return reply("Rejected");
        out.state = std::move(bytes);
        return out;
    }
    const bool same = session_ && session_->token == req.lock;
    if (req.command == "sound_unlock") {
        if (!same) return reply("AlreadyReleased");
        release();
        return reply("Released");
    }
    if (!same) return reply("Unlocked");
    if (req.command == "sound_undo_locked") {
        if (!session_->undo) return reply("NothingToUndo");
        if (!plugin_.set(*session_->undo)) return reply("Rejected");
        session_->undo.reset();
        ++session_->generation;
        return reply("Undone");
    }
    if (session_->generation != req.generation) return reply("Changed");
    if (req.command == "sound_replace_locked") {
        SoundBytes before;
        if (!plugin_.get(before) || !plugin_.set(req.state)) return reply("Rejected");
        session_->undo = std::move(before);
        ++session_->generation;
        return reply("Replaced");
    }
    if (req.command == "sound_preset_commit") {
        if (!presets_.matches(req.guard)) return reply("Changed");
        const bool ok = presets_.commit();
        session_->guard = presets_.current();
        return reply(ok ? "Committed" : "Rejected");
    }
    return reply("Rejected");
}
namespace {
constexpr auto lock_message = L"AI アプリが編集しています。この間は操作できません";
constexpr auto button_text = L"ロックを解除";
constexpr auto overlay_class = L"MivSoundLockOverlay";
constexpr auto overlay_property = L"MivSoundLockOverlayOwner";
constexpr int unlock_button_id = 1;
constexpr UINT_PTR blink_timer = 1;
HFONT band_font(int height) {
    return CreateFontW(-height, 0, 0, 0, FW_NORMAL, FALSE, FALSE, FALSE, DEFAULT_CHARSET,
        OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, DEFAULT_PITCH, L"Yu Gothic UI");
}
}
void apply_sound_lock_window(HWND frame, HWND child, bool locked) {
    if (!frame || !IsWindow(frame)) return;
    const bool had_focus = child && (GetFocus() == child || IsChild(child, GetFocus()));
    if (child && IsWindow(child)) EnableWindow(child, !locked);
    SetWindowTextW(frame, locked ? L"mIV Sound — AI が編集中" : L"mIV Sound");
    if (locked && had_focus) SetFocus(frame);
    InvalidateRect(frame, nullptr, FALSE);
}
SoundLockOverlay::SoundLockOverlay(HWND frame, std::function<void()> unlock)
    : frame_(frame), unlock_(std::move(unlock)) {
    WNDCLASSW klass{};
    klass.hInstance = GetModuleHandleW(nullptr);
    klass.lpfnWndProc = proc;
    klass.hCursor = LoadCursorW(nullptr, IDC_ARROW);
    klass.lpszClassName = overlay_class;
    if (!RegisterClassW(&klass) && GetLastError() != ERROR_CLASS_ALREADY_EXISTS) return;
    hwnd_ = CreateWindowExW(WS_EX_CONTROLPARENT, overlay_class, lock_message,
        WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS, 0, 0, 1, 1,
        frame, nullptr, klass.hInstance, this);
    if (!hwnd_) return;
    button_ = CreateWindowExW(0, L"BUTTON", button_text,
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON,
        0, 0, 1, 1, hwnd_, reinterpret_cast<HMENU>(INT_PTR(unlock_button_id)), klass.hInstance, nullptr);
    if (!button_) { DestroyWindow(hwnd_); hwnd_ = nullptr; return; }
    SetPropW(frame_, overlay_property, hwnd_);
    layout();
    ShowWindow(hwnd_, SW_SHOWNOACTIVATE);
}
SoundLockOverlay::~SoundLockOverlay() {
    if (GetPropW(frame_, overlay_property) == hwnd_) RemovePropW(frame_, overlay_property);
    if (hwnd_ && IsWindow(hwnd_)) DestroyWindow(hwnd_);
    if (font_) DeleteObject(font_);
}
void SoundLockOverlay::layout() {
    if (!hwnd_ || !button_ || IsIconic(frame_)) return;
    RECT client{};
    if (!GetClientRect(frame_, &client) || client.right <= 0) return;
    const UINT dpi = GetDpiForWindow(frame_);
    const int scale = dpi ? int(dpi) : 96;
    const int pad = std::max(1, MulDiv(8, scale, 96));
    HFONT font = band_font(std::max(1, MulDiv(15, scale, 96)));
    HDC dc = GetDC(hwnd_);
    auto old = SelectObject(dc, font);
    RECT button_measure{}, text_measure{};
    DrawTextW(dc, button_text, -1, &button_measure, DT_CALCRECT | DT_SINGLELINE);
    DrawTextW(dc, lock_message, -1, &text_measure, DT_CALCRECT | DT_SINGLELINE);
    const int button_width = button_measure.right + 2 * pad;
    const int button_height = button_measure.bottom + 2 * pad;
    const int width = std::max(1, std::min<int>(client.right, text_measure.right + button_width + 3 * pad));
    const bool stacked = width < button_width + 12 * pad;
    text_measure = {0, 0, std::max(1, width - (stacked ? 2 * pad : button_width + 3 * pad)), 0};
    DrawTextW(dc, lock_message, -1, &text_measure, DT_CALCRECT | DT_WORDBREAK);
    SelectObject(dc, old); ReleaseDC(hwnd_, dc);
    SendMessageW(button_, WM_SETFONT, reinterpret_cast<WPARAM>(font), TRUE);
    if (font_) DeleteObject(font_);
    font_ = font;
    const int height = stacked ? text_measure.bottom + button_height + 3 * pad
                               : std::max<int>(text_measure.bottom, button_height) + 2 * pad;
    text_ = {pad, pad, std::max(pad + 1, width - (stacked ? pad : button_width + 2 * pad)),
             stacked ? pad + text_measure.bottom : height - pad};
    const int button_x = stacked ? std::max(0, (width - button_width) / 2) : width - button_width - pad;
    const int button_y = stacked ? text_.bottom + pad : (height - button_height) / 2;
    SetWindowPos(button_, nullptr, button_x, button_y, std::min(button_width, width), button_height,
        SWP_NOZORDER | SWP_NOACTIVATE);
    SetWindowPos(hwnd_, HWND_TOP, (client.right - width) / 2, editor_status_height(dpi), width, height,
        SWP_NOACTIVATE);
    InvalidateRect(hwnd_, nullptr, TRUE);
}
void SoundLockOverlay::focus() {
    if (button_ && IsWindow(button_)) SetFocus(button_);
}
void SoundLockOverlay::blink() {
    if (!hwnd_) return;
    blinking_ = true;
    InvalidateRect(hwnd_, nullptr, TRUE);
    SetTimer(hwnd_, blink_timer, 140, nullptr);
}
bool SoundLockOverlay::translate(MSG& message) {
    HWND root = GetAncestor(message.hwnd, GA_ROOT);
    HWND overlay = static_cast<HWND>(GetPropW(root, overlay_property));
    return overlay && IsWindowVisible(overlay) && IsDialogMessageW(overlay, &message);
}
LRESULT CALLBACK SoundLockOverlay::proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    if (msg == WM_NCCREATE) {
        auto* create = reinterpret_cast<CREATESTRUCTW*>(lparam);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(create->lpCreateParams));
    }
    auto* self = reinterpret_cast<SoundLockOverlay*>(GetWindowLongPtrW(hwnd, GWLP_USERDATA));
    if (!self) return DefWindowProcW(hwnd, msg, wparam, lparam);
    if (msg == DM_GETDEFID) return MAKELONG(unlock_button_id, DC_HASDEFID);
    if (msg == WM_COMMAND && LOWORD(wparam) == unlock_button_id && HIWORD(wparam) == BN_CLICKED) {
        // The callback may destroy this overlay as part of releasing the lock.
        auto unlock = self->unlock_;
        unlock();
        return 0;
    }
    if (msg == WM_SETFOCUS) { self->focus(); return 0; }
    if (msg == WM_LBUTTONDOWN || msg == WM_RBUTTONDOWN || msg == WM_MBUTTONDOWN) {
        self->blink(); self->focus(); return 0;
    }
    if (msg == WM_TIMER && wparam == blink_timer) {
        KillTimer(hwnd, blink_timer);
        self->blinking_ = false;
        InvalidateRect(hwnd, nullptr, TRUE);
        return 0;
    }
    if (msg == WM_ERASEBKGND) return 1;
    if (msg == WM_PAINT) {
        PAINTSTRUCT paint{}; HDC dc = BeginPaint(hwnd, &paint);
        RECT client{}; GetClientRect(hwnd, &client);
        HBRUSH brush = CreateSolidBrush(self->blinking_ ? RGB(110, 100, 48) : RGB(48, 48, 32));
        FillRect(dc, &client, brush); DeleteObject(brush);
        auto old = SelectObject(dc, self->font_);
        SetBkMode(dc, TRANSPARENT); SetTextColor(dc, RGB(240, 240, 220));
        auto text = self->text_;
        DrawTextW(dc, lock_message, -1, &text, DT_WORDBREAK);
        SelectObject(dc, old); EndPaint(hwnd, &paint);
        return 0;
    }
    return DefWindowProcW(hwnd, msg, wparam, lparam);
}
}
