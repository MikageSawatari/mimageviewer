#pragma once
#include <cstdint>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <vector>
#include <windows.h>

namespace miv {
using SoundBytes = std::vector<uint8_t>;
struct SoundGuard {
    enum class Kind { Absent, Held, Unavailable } kind = Kind::Absent;
    uint64_t epoch = 0;
};
struct SoundRequest {
    std::string command, lock;
    uint64_t generation = 0;
    SoundGuard guard;
    SoundBytes state;
};
struct SoundReply {
    std::string status;
    std::optional<std::string> lock;
    uint64_t generation = 0;
    SoundGuard guard;
    std::optional<SoundBytes> state;
};
// All methods, including destruction, belong to the slot GUI thread. These
// boundaries deliberately contain no audio-thread fence or worker wait.
struct SoundPlugin {
    virtual ~SoundPlugin() = default;
    virtual bool eligible() const = 0;
    virtual bool get(SoundBytes&) = 0;
    virtual bool set(const SoundBytes&) = 0;
    virtual void show_lock(bool) = 0;
};
struct SoundPresetBoundary {
    virtual ~SoundPresetBoundary() = default;
    virtual SoundGuard acquire() = 0;
    virtual bool matches(SoundGuard) const = 0;
    virtual bool commit() = 0;
    virtual SoundGuard current() const = 0;
    virtual void release() = 0;
};
// Only fixed environment-derived paths; no caller-supplied paths, content reads
// or hashes on the GUI thread. A held handle shares reads but forbids writes/delete.
class SoundPresetFile final : public SoundPresetBoundary {
public:
    explicit SoundPresetFile(std::wstring data_dir);
    ~SoundPresetFile() override;
    SoundGuard acquire() override;
    bool matches(SoundGuard) const override;
    bool commit() override;
    SoundGuard current() const override { return guard_; }
    void release() override;
    static std::wstring environment_directory();
private:
    std::wstring path_, temporary_;
    HANDLE handle_ = INVALID_HANDLE_VALUE;
    uint64_t epoch_ = 0;
    SoundGuard guard_;
};
class SoundLock {
public:
    SoundLock(SoundPlugin& plugin, SoundPresetBoundary& presets,
              std::function<std::string()> token);
    SoundReply execute(const SoundRequest&);
    std::optional<std::string> release();
    bool locked() const { return session_.has_value(); }
private:
    struct Session {
        std::string token;
        uint64_t generation = 0;
        SoundGuard guard;
        std::optional<SoundBytes> undo;
    };
    SoundReply reply(const char*) const;
    SoundPlugin& plugin_;
    SoundPresetBoundary& presets_;
    std::function<std::string()> token_;
    std::optional<Session> session_;
};

void apply_sound_lock_window(HWND frame, HWND child, bool locked);
// A sibling of plugin_host, never part of its allocation. Standard BUTTON
// semantics (focus, Tab, Enter/Space) remain outside the application's keymap.
class SoundLockOverlay {
public:
    SoundLockOverlay(HWND frame, std::function<void()> unlock);
    ~SoundLockOverlay();
    SoundLockOverlay(const SoundLockOverlay&) = delete;
    SoundLockOverlay& operator=(const SoundLockOverlay&) = delete;
    void layout();
    void focus();
    void blink();
    HWND hwnd() const { return hwnd_; }
    HWND button() const { return button_; }
    static bool translate(MSG& message);
private:
    static LRESULT CALLBACK proc(HWND, UINT, WPARAM, LPARAM);
    HWND frame_ = nullptr, hwnd_ = nullptr, button_ = nullptr;
    HFONT font_ = nullptr;
    RECT text_{};
    bool blinking_ = false;
    std::function<void()> unlock_;
};
}
