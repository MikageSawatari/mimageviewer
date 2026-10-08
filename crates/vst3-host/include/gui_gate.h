#pragma once
#include <atomic>
#include <cstdint>
#include <cstddef>
#include <string>
#include <windows.h>

namespace miv {
struct GuiGateSnapshot {
    uint64_t minimized_sequence;
    uint64_t remote;
    uint64_t auto_presentation = 0;
    bool auto_video = false;
    uint64_t auto_revision = 0;
    constexpr bool permits(const GuiGateSnapshot& current, bool minimized, bool main_visible = true) const {
        return !minimized && !(current.remote & 1) &&
               minimized_sequence == current.minimized_sequence && remote == current.remote &&
               (!auto_video || (main_visible && (current.auto_presentation & 31) == 0 &&
                                auto_revision == (current.auto_presentation >> 5)));
    }
};
// Transport layout matches Rust GateState. No visibility state or pointers.
struct GuiGateState {
    uint64_t magic;
    uint64_t version;
    std::atomic<uint64_t> minimized_sequence;
    std::atomic<uint64_t> remote;
    std::atomic<uint64_t> keep_visible_when_minimized;
    std::atomic<uint64_t> auto_presentation;
};
static_assert(sizeof(GuiGateState) == 48);
static_assert(offsetof(GuiGateState, keep_visible_when_minimized) == 32);
static_assert(offsetof(GuiGateState, auto_presentation) == 40);
static_assert(std::atomic<uint64_t>::is_always_lock_free);
class GuiGateReader {
public:
    ~GuiGateReader() {
        if (state_) UnmapViewOfFile(state_);
        if (handle_) CloseHandle(handle_);
    }
    bool open(const std::wstring& name) {
        if (handle_ || name.empty()) return false;
        handle_ = OpenFileMappingW(FILE_MAP_READ, FALSE, name.c_str());
        if (!handle_) return false;
        state_ = static_cast<const GuiGateState*>(MapViewOfFile(handle_, FILE_MAP_READ, 0, 0, sizeof(GuiGateState)));
        return state_ && state_->magic == 0x4d49564741544501ULL && state_->version == 3;
    }
    GuiGateSnapshot snapshot() const {
        return {state_->minimized_sequence.load(std::memory_order_acquire), state_->remote.load(std::memory_order_acquire),
                state_->auto_presentation.load(std::memory_order_acquire)};
    }
    bool keep_visible_when_minimized() const {
        return state_->keep_visible_when_minimized.load(std::memory_order_acquire) != 0;
    }
private:
    HANDLE handle_ = nullptr;
    const GuiGateState* state_ = nullptr;
};
}
