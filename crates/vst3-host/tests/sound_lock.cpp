#include "sound_lock.h"
#include "gui_task_queue.h"
#include "gui_frame.h"
#include <cstdio>
#include <cstdlib>
#include <cstring>

using namespace miv;
namespace {
void check(bool ok, const char* label) {
    if (!ok) {
        std::fprintf(stderr, "sound lock failure: %s (Win32=%lu)\n", label, GetLastError());
        std::exit(1);
    }
}
struct Plugin final : SoundPlugin {
    bool allowed = true, reject = false, fail_get = false, locked = false;
    unsigned get_calls = 0, set_calls = 0;
    SoundBytes bytes{1};
    DWORD owner = GetCurrentThreadId();
    bool eligible() const override { return allowed; }
    bool get(SoundBytes& out) override {
        check(GetCurrentThreadId() == owner, "get on GUI owner");
        ++get_calls; out = bytes; return !fail_get;
    }
    bool set(const SoundBytes& in) override {
        check(GetCurrentThreadId() == owner, "set on GUI owner");
        ++set_calls;
        if (reject) return false;
        bytes = in; return true;
    }
    void show_lock(bool value) override {
        check(GetCurrentThreadId() == owner, "window updates on GUI owner");
        locked = value;
    }
};
struct Presets final : SoundPresetBoundary {
    SoundGuard guard;
    uint64_t epoch = 0;
    bool exists = true, held = false, fail_reopen = false;
    unsigned commits = 0;
    SoundGuard acquire() override {
        held = exists;
        return guard = {held ? SoundGuard::Kind::Held : SoundGuard::Kind::Absent, held ? ++epoch : 0};
    }
    SoundGuard current() const override { return guard; }
    bool matches(SoundGuard expected) const override {
        return expected.kind == guard.kind &&
            (held ? expected.epoch == epoch : !exists && expected.kind == SoundGuard::Kind::Absent);
    }
    bool commit() override {
        ++commits;
        exists = true;
        if (fail_reopen) guard = {SoundGuard::Kind::Unavailable, 0};
        else acquire();
        return true;
    }
    void release() override { held = false; guard = {}; }
};
SoundRequest request(const char* command, const std::string& token = {}, uint64_t generation = 0) {
    SoundRequest r;
    r.command = command; r.lock = token; r.generation = generation;
    return r;
}
void state_tests() {
    Plugin plugin; Presets presets; unsigned serial = 0;
    SoundLock lock(plugin, presets, [&] { return std::to_string(++serial); });
    plugin.allowed = false;
    for (const auto* cmd : {"sound_lock", "sound_get", "sound_replace_locked", "sound_undo_locked", "sound_preset_commit", "sound_unlock"})
        check(lock.execute(request(cmd)).status == "Rejected", "user VST rejected for every command");
    check(plugin.get_calls == 0 && plugin.set_calls == 0, "user VST never touched");
    plugin.allowed = true;
    check(lock.execute(request("sound_get")).lock == std::nullopt, "unlocked read has no revision");
    auto acquired = lock.execute(request("sound_lock"));
    check(acquired.status == "Locked" && acquired.lock && acquired.generation == 0 && plugin.locked, "lock generation zero");
    const auto token = *acquired.lock;
    check(lock.execute(request("sound_lock")).status == "Busy", "second lock busy");
    auto replace = request("sound_replace_locked", "wrong", 0); replace.state = {2};
    check(lock.execute(replace).status == "Unlocked", "wrong token");
    replace.lock = token; replace.generation = 9;
    check(lock.execute(replace).status == "Changed", "stale generation");
    replace.generation = 0;
    plugin.reject = true;
    check(lock.execute(replace).generation == 0, "setState rejection keeps generation");
    check(lock.execute(request("sound_undo_locked", token)).status == "NothingToUndo", "failed initial write creates no undo");
    plugin.reject = false;
    check(lock.execute(replace).status == "Replaced" && plugin.bytes == SoundBytes{2}, "replace success");
    plugin.reject = true; replace.generation = 1; replace.state = {3};
    check(lock.execute(replace).status == "Rejected", "second setState rejection");
    plugin.reject = false;
    auto undone = lock.execute(request("sound_undo_locked", token));
    check(undone.status == "Undone" && undone.generation == 2 && plugin.bytes == SoundBytes{1}, "rejection preserves prior undo and generation");
    check(lock.execute(request("sound_undo_locked", token)).status == "NothingToUndo", "undo consumed once");
    replace.generation = 2; check(lock.execute(replace).status == "Replaced", "next replace");
    plugin.reject = true;
    check(lock.execute(request("sound_undo_locked", token)).status == "Rejected", "failed undo preserves state");
    plugin.reject = false;
    check(lock.execute(request("sound_undo_locked", token)).status == "Undone", "failed undo remains available");
    check(lock.execute(request("sound_unlock", "wrong")).status == "AlreadyReleased" && plugin.locked, "foreign unlock no effect");
    check(lock.execute(request("sound_unlock", token)).status == "Released" && !plugin.locked && !presets.held, "AI release closes guard");
    acquired = lock.execute(request("sound_lock"));
    check(acquired.lock != token && acquired.generation == 0, "new lock new token and generation");
    check(lock.execute(request("sound_undo_locked", *acquired.lock)).status == "NothingToUndo", "new lock no undo");
    // Simulate both producers on the same owner queue. No audio fence, process
    // toggle or acknowledgement exists in this boundary or fake component.
    GuiTaskQueue queue;
    queue.push([&] { check(lock.release() == acquired.lock, "user release first"); });
    queue.push([&] { check(lock.execute(request("sound_replace_locked", *acquired.lock)).status == "Unlocked", "later write rejected"); });
    queue.drain();
    acquired = lock.execute(request("sound_lock"));
    queue.push([&] {
        auto write = request("sound_replace_locked", *acquired.lock); write.state = {5};
        check(lock.execute(write).status == "Replaced", "write before user release completes");
    });
    queue.push([&] { lock.release(); });
    queue.drain();
    check(plugin.bytes == SoundBytes{5} && !lock.locked(), "ordered write/release complete");
}
void guard_state_tests() {
    Plugin plugin; Presets presets;
    SoundLock lock(plugin, presets, [] { return "token"; });
    auto acquired = lock.execute(request("sound_lock"));
    auto commit = request("sound_preset_commit", *acquired.lock);
    commit.guard = acquired.guard; ++commit.guard.epoch;
    check(lock.execute(commit).status == "Changed" && presets.commits == 0, "epoch mismatch never writes");
    commit.guard = acquired.guard; commit.generation = 1;
    check(lock.execute(commit).status == "Changed", "commit generation mismatch");
    commit.generation = 0;
    auto result = lock.execute(commit);
    check(result.status == "Committed" && result.guard.epoch > acquired.guard.epoch, "commit advances guard epoch");
    check(lock.execute(commit).status == "Changed", "old epoch cannot commit twice");
    commit.guard = result.guard; presets.fail_reopen = true;
    check(lock.execute(commit).status == "Committed", "successful replace remains success on guard reopen error");
    auto read = lock.execute(request("sound_get"));
    check(read.status == "State" && read.state && read.guard.kind == SoundGuard::Kind::Unavailable, "guard loss preserves state reads");
    lock.release(); presets.fail_reopen = false; presets.exists = false;
    acquired = lock.execute(request("sound_lock"));
    commit.guard = acquired.guard;
    check(commit.guard.kind == SoundGuard::Kind::Absent, "missing preset guard absent");
    presets.exists = true;
    check(lock.execute(commit).status == "Changed", "new file after absent rejects commit");
    lock.release();
}
void write_file(const std::wstring& path, const char* value) {
    HANDLE file = CreateFileW(path.c_str(), GENERIC_WRITE, 0, nullptr, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    check(file != INVALID_HANDLE_VALUE, "create disposable file");
    DWORD written = 0;
    check(WriteFile(file, value, DWORD(std::strlen(value)), &written, nullptr) != FALSE, "write disposable file");
    check(FlushFileBuffers(file) != FALSE, "flush disposable file");
    CloseHandle(file);
}
void file_tests() {
    wchar_t temp[MAX_PATH + 1]{};
    check(GetTempPathW(MAX_PATH, temp) != 0, "temporary directory");
    // GetTempFileName creates a uniquely owned disposable path. Never use the
    // environment/profile or real preset data for handle tests.
    wchar_t unique[MAX_PATH + 1]{};
    check(GetTempFileNameW(temp, L"miv", 0, unique) != 0, "unique disposable path");
    check(DeleteFileW(unique) != FALSE && CreateDirectoryW(unique, nullptr) != FALSE, "disposable root");
    const std::wstring root(unique), dir = root + L"\\plugin";
    check(CreateDirectoryW(dir.c_str(), nullptr) != FALSE, "disposable plugin directory");
    const auto path = dir + L"\\effetune_presets.json";
    const auto temporary = path + L".miv-mcp.tmp";
    const auto user_temporary = path + L".user.tmp";
    write_file(path, "original"); write_file(user_temporary, "user");
    {
        SoundPresetFile file(root);
        auto guard = file.acquire();
        check(guard.kind == SoundGuard::Kind::Held && file.matches(guard), "real handle held");
        check(!MoveFileExW(user_temporary.c_str(), path.c_str(), MOVEFILE_REPLACE_EXISTING), "guard blocks replacement");
        HANDLE reader = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
        check(reader != INVALID_HANDLE_VALUE, "guard permits reader"); CloseHandle(reader);
        write_file(temporary, "AI");
        check(file.commit(), "guard closes around fixed temporary replace");
        const auto after = file.current();
        check(after.kind == SoundGuard::Kind::Held && after.epoch > guard.epoch && !file.matches(guard), "real guard epoch advanced");
        check(!MoveFileExW(user_temporary.c_str(), path.c_str(), MOVEFILE_REPLACE_EXISTING), "replacement guarded again");
        file.release();
        check(MoveFileExW(user_temporary.c_str(), path.c_str(), MOVEFILE_REPLACE_EXISTING) != FALSE, "unlock permits replacement");
        check(DeleteFileW(path.c_str()) != FALSE, "remove own fixture");
        guard = file.acquire();
        check(guard.kind == SoundGuard::Kind::Absent && file.matches(guard), "absent real guard");
        write_file(path, "created later");
        check(!file.matches(guard), "absent real guard catches creation");
    }
    check(DeleteFileW(path.c_str()) != FALSE, "delete fixture only");
    check(RemoveDirectoryW(dir.c_str()) != FALSE && RemoveDirectoryW(root.c_str()) != FALSE, "remove exact disposable directories");
}
struct ModalContext { GuiTaskQueue queue; bool modal = false, ran = false, swallowed = false; };
LRESULT CALLBACK modal_proc(HWND hwnd, UINT message, WPARAM wparam, LPARAM lparam) {
    if (message != WM_APP + 1) return DefWindowProcW(hwnd, message, wparam, lparam);
    auto& context = *reinterpret_cast<ModalContext*>(lparam);
    context.modal = true;
    context.queue.push([&context] { check(!context.modal, "task runs after modal exits"); context.ran = true; });
    check(PostThreadMessageW(GetCurrentThreadId(), WM_APP + 2, 0, 0) != FALSE, "queue notification");
    MSG nested{};
    do {
        check(GetMessageW(&nested, nullptr, 0, 0) > 0, "nested get message");
        if (nested.hwnd) DispatchMessageW(&nested);
    } while (nested.hwnd != nullptr || nested.message != WM_APP + 2);
    check(nested.hwnd == nullptr && nested.message == WM_APP + 2, "nested loop consumes thread wake");
    DispatchMessageW(&nested); // Win32 silently discards the thread message.
    context.swallowed = true;
    check(!context.ran, "not drained inside nested loop");
    context.modal = false;
    return 0;
}
void modal_and_window_tests() {
    WNDCLASSW klass{};
    klass.hInstance = GetModuleHandleW(nullptr); klass.lpfnWndProc = modal_proc;
    klass.lpszClassName = L"MivSoundLockInertTests";
    check(RegisterClassW(&klass) != 0, "inert window class");
    HWND frame = CreateWindowW(klass.lpszClassName, L"mIV Sound", WS_OVERLAPPEDWINDOW,
        0, 0, 800, 400, nullptr, nullptr, klass.hInstance, nullptr);
    check(frame != nullptr, "inert hidden frame");
    HWND child = CreateWindowW(L"STATIC", L"", WS_CHILD, 0, 0, 600, 300, frame, nullptr, klass.hInstance, nullptr);
    check(child != nullptr, "inert plugin child");
    apply_sound_lock_window(nullptr, nullptr, true); // Lock may precede HWND creation.
    apply_sound_lock_window(frame, child, true);
    check(!IsWindowEnabled(child), "late-created plugin input disabled");
    wchar_t title[100]{}; GetWindowTextW(frame, title, 100);
    check(std::wstring(title) == L"mIV Sound — AI が編集中", "locked title");
    RECT child_before{}; GetWindowRect(child, &child_before);
    bool user_unlocked = false;
    {
        SoundLockOverlay overlay(frame, [&] { user_unlocked = true; });
        check(overlay.hwnd() && overlay.button() && GetParent(overlay.hwnd()) == frame &&
            GetParent(overlay.button()) == overlay.hwnd(), "dedicated child overlay and real button");
        check((GetWindowLongPtrW(overlay.button(), GWL_STYLE) & WS_TABSTOP) != 0, "unlock is a tab stop");
        check(SendMessageW(overlay.button(), WM_GETFONT, 0, 0) != 0, "button uses measured DPI font");
        RECT band{}, button_rect{}, allocated{}, frame_client{};
        GetWindowRect(overlay.hwnd(), &band); GetWindowRect(overlay.button(), &button_rect);
        GetClientRect(frame, &frame_client); GetWindowRect(child, &allocated);
        check(EqualRect(&child_before, &allocated) != FALSE, "overlay preserves plugin allocation");
        check(button_rect.left >= band.left && button_rect.top >= band.top &&
            button_rect.right <= band.right && button_rect.bottom <= band.bottom, "real button fits measured overlay");
        check(band.right - band.left <= frame_client.right, "overlay fits frame");
        overlay.blink();
        SendMessageW(overlay.hwnd(), WM_TIMER, 1, 0); // Ends exactly one blink, no wait.
        SetWindowPos(frame, nullptr, 0, 0, 480, 300, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        overlay.layout();
        GetWindowRect(child, &allocated);
        check(EqualRect(&child_before, &allocated) != FALSE, "resize layout leaves plugin allocation owner unchanged");
        SendMessageW(overlay.button(), BM_CLICK, 0, 0);
        check(user_unlocked, "native button reaches user release callback");
    }
    apply_sound_lock_window(frame, child, false);
    check(IsWindowEnabled(child) != FALSE, "unlock restores input");
    GetWindowTextW(frame, title, 100);
    check(std::wstring(title) == L"mIV Sound", "unlock restores title");
    ModalContext context;
    MSG ensure_queue{}; PeekMessageW(&ensure_queue, nullptr, WM_USER, WM_USER, PM_NOREMOVE);
    // An actual outer DispatchMessage enters an actual nested Win32 loop. No
    // windows are shown and no user input, plugin or product process is used.
    check(PostMessageW(frame, WM_APP + 1, 0, reinterpret_cast<LPARAM>(&context)) != FALSE, "post modal fixture");
    MSG outer{};
    while (GetMessageW(&outer, nullptr, 0, 0) > 0) {
        const bool fixture = outer.hwnd == frame && outer.message == WM_APP + 1;
        context.queue.dispatch_outer([&] { DispatchMessageW(&outer); });
        if (fixture) break;
    }
    check(context.swallowed && context.ran, "outer return drains despite no further wake");
    DestroyWindow(frame);
    check(UnregisterClassW(klass.lpszClassName, klass.hInstance) != FALSE, "release inert window class");
}
}
int main() {
    state_tests(); guard_state_tests(); file_tests(); modal_and_window_tests();
    std::puts("sound lock boundaries passed");
    return 0;
}
