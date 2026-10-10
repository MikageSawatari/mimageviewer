#pragma once

#include "protocol.h"
#include "ipc_strings.h"
#include "utf8_paths.h"
#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <future>
#include <functional>
#include <memory>
#include <mutex>
#include <optional>
#include <string_view>
#include <unordered_map>
#include <vector>

namespace miv {
constexpr size_t NOW_PLAYING_IMAGE_MAX = 8388608;
constexpr size_t NOW_PLAYING_WIRE_MAX = 11184812;
constexpr uint64_t NOW_PLAYING_TIMEOUT_MS = 5000;
constexpr DWORD NOW_PLAYING_TIMEOUT_EXIT = 0xEFFEC002u;

struct NowPlayingSnapshot {
    uint64_t slot_id = 0, revision = 0;
    std::wstring title, album, artist, mime;
    std::vector<uint8_t> artwork;
};

inline void now_playing_space(const std::string& json, size_t& p) {
    while (p < json.size() && (json[p] == ' ' || json[p] == '\t' || json[p] == '\r' || json[p] == '\n')) ++p;
}

// JSON string tokens remain views into the single bounded reader frame. In
// particular, the 11 MiB artwork string is never copied into a second string.
inline bool now_playing_string_view(const std::string& json, size_t& p, std::string_view& out) {
    if (p == json.size() || json[p++] != '"') return false;
    const size_t start = p - 1;
    while (p < json.size()) {
        const unsigned char c = static_cast<unsigned char>(json[p++]);
        if (c == '"') { out = std::string_view(json).substr(start, p - start); return true; }
        if (c < 0x20) return false;
        if (c == '\\') {
            if (p == json.size()) return false;
            ++p; // Full escape validation is done by the bounded decoder below.
        }
    }
    return false;
}

template<class Emit>
bool now_playing_json_chars(std::string_view token, Emit emit) {
    if (token.size() < 2 || token.front() != '"' || token.back() != '"') return false;
    for (size_t i = 1; i + 1 < token.size(); ++i) {
        unsigned char c = static_cast<unsigned char>(token[i]);
        if (c < 0x20 || c == '"') return false;
        if (c == '\\') {
            if (++i + 1 >= token.size()) return false;
            switch (token[i]) {
            case '"': c = '"'; break;
            case '\\': c = '\\'; break;
            case '/': c = '/'; break;
            case 'b': c = '\b'; break;
            case 'f': c = '\f'; break;
            case 'n': c = '\n'; break;
            case 'r': c = '\r'; break;
            case 't': c = '\t'; break;
            case 'u': {
                unsigned value = 0;
                for (int n = 0; n < 4; ++n) {
                    if (++i + 1 >= token.size()) return false;
                    const char h = token[i];
                    const int v = h >= '0' && h <= '9' ? h - '0' :
                        h >= 'a' && h <= 'f' ? h - 'a' + 10 : h >= 'A' && h <= 'F' ? h - 'A' + 10 : -1;
                    if (v < 0) return false;
                    value = value * 16 + static_cast<unsigned>(v);
                }
                // Artwork alphabet is ASCII; no Unicode expansion buffer.
                if (value > 127) return false;
                c = static_cast<unsigned char>(value);
                break;
            }
            default: return false;
            }
        }
        if (!emit(c)) return false;
    }
    return true;
}
inline int now_playing_b64(unsigned char c) {
    if (c >= 'A' && c <= 'Z') return c - 'A';
    if (c >= 'a' && c <= 'z') return c - 'a' + 26;
    if (c >= '0' && c <= '9') return c - '0' + 52;
    return c == '+' ? 62 : c == '/' ? 63 : -1;
}
inline bool now_playing_decode_artwork(std::string_view token, std::vector<uint8_t>& out) {
    size_t count = 0, padding = 0;
    int last = 0;
    if (!now_playing_json_chars(token, [&](unsigned char c) {
        if (++count > NOW_PLAYING_WIRE_MAX) return false;
        if (c == '=') { return ++padding <= 2; }
        const int v = now_playing_b64(c);
        if (v < 0 || padding != 0) return false;
        last = v;
        return true;
    })) return false;
    if (count % 4 != 0 || (count == 0 && padding) ||
        (padding == 1 && (last & 3)) || (padding == 2 && (last & 15))) return false;
    const size_t decoded = count / 4 * 3 - padding;
    if (decoded > NOW_PLAYING_IMAGE_MAX) return false; // Before allocation.
    out.clear();
    out.reserve(decoded);
    uint32_t bits = 0;
    int available = 0;
    const bool ok = now_playing_json_chars(token, [&](unsigned char c) {
        if (c == '=') return true;
        bits = (bits << 6) | static_cast<uint32_t>(now_playing_b64(c));
        available += 6;
        if (available >= 8) {
            available -= 8;
            if (out.size() >= decoded || out.size() >= NOW_PLAYING_IMAGE_MAX) return false;
            out.push_back(static_cast<uint8_t>(bits >> available));
        }
        return true;
    });
    return ok && out.size() == decoded;
}
inline bool now_playing_uint(const std::string& json, size_t& p, uint64_t& value) {
    if (p == json.size() || json[p] < '0' || json[p] > '9') return false;
    value = 0;
    const size_t start = p;
    while (p < json.size() && json[p] >= '0' && json[p] <= '9') {
        const unsigned digit = static_cast<unsigned>(json[p++] - '0');
        if (value > (UINT64_MAX - digit) / 10) return false;
        value = value * 10 + digit;
    }
    return p - start == 1 || json[start] != '0';
}
inline bool now_playing_text(const std::string& json, size_t& p, std::wstring& out, bool truncate = true) {
    std::string text;
    if (!json_string(json, p, text)) return false;
    try { out = utf8_to_utf16(text); } catch (const std::exception&) { return false; }
    if (truncate && out.size() > 4096) {
        out.resize(4096);
        if (out.back() >= 0xd800 && out.back() <= 0xdbff) out.pop_back();
    }
    return true;
}
inline bool parse_now_playing(const std::string& json, NowPlayingSnapshot& result) {
    if (json.size() > MAX_CONTROL_MSG_SIZE) return false;
    size_t p = 0;
    unsigned seen = 0;
    std::string_view art;
    now_playing_space(json, p);
    if (p == json.size() || json[p++] != '{') return false;
    for (unsigned field = 0; field < 8; ++field) {
        now_playing_space(json, p);
        std::string name;
        if (!json_string(json, p, name)) return false;
        now_playing_space(json, p);
        if (p == json.size() || json[p++] != ':') return false;
        now_playing_space(json, p);
        unsigned bit = 0;
        bool ok = false;
        if (name == "cmd" && field == 0) {
            std::string cmd;
            bit = 1; ok = json_string(json, p, cmd) && cmd == "set_now_playing";
        } else if (name == "slot_id") { bit = 2; ok = now_playing_uint(json, p, result.slot_id); }
        else if (name == "revision") { bit = 4; ok = now_playing_uint(json, p, result.revision); }
        else if (name == "title") { bit = 8; ok = now_playing_text(json, p, result.title); }
        else if (name == "album") { bit = 16; ok = now_playing_text(json, p, result.album); }
        else if (name == "artist") { bit = 32; ok = now_playing_text(json, p, result.artist); }
        else if (name == "artwork") { bit = 64; ok = now_playing_string_view(json, p, art); }
        else if (name == "artworkMimeType") { bit = 128; ok = now_playing_text(json, p, result.mime, false); }
        if (!ok || (seen & bit)) return false;
        seen |= bit;
        now_playing_space(json, p);
        if (p == json.size() || json[p++] != (field == 7 ? '}' : ',')) return false;
    }
    now_playing_space(json, p);
    if (p != json.size() || seen != 255 || !now_playing_decode_artwork(art, result.artwork)) return false;
    return result.artwork.empty() ? result.mime.empty() :
        result.mime == L"image/png" || result.mime == L"image/jpeg" ||
        result.mime == L"image/gif" || result.mime == L"image/webp";
}

// The single owner spans reader, control and GUI. Scheduled means exactly one
// token across the two queues; Running can retain exactly one latest pending.
class NowPlayingMailbox {
public:
    enum class Phase { Idle, Scheduled, Running, Closed };
    explicit NowPlayingMailbox(uint64_t id) : slot_id(id) {}
    const uint64_t slot_id;
    bool admit(std::shared_ptr<const NowPlayingSnapshot> snapshot) {
        std::lock_guard<std::mutex> lock(mutex_);
        if (phase_ == Phase::Closed || (watermark_ && snapshot->revision <= *watermark_)) return false;
        watermark_ = snapshot->revision;
        pending_ = std::move(snapshot);
        if (phase_ != Phase::Idle) return false;
        phase_ = Phase::Scheduled;
        return true;
    }
    std::shared_ptr<const NowPlayingSnapshot> begin(uint64_t now) {
        std::lock_guard<std::mutex> lock(mutex_);
        if (phase_ != Phase::Scheduled || !pending_) return {};
        phase_ = Phase::Running;
        ++execution_count_;
        running_ = std::move(pending_);
        started_ = now;
        return running_;
    }
    bool finish() {
        std::lock_guard<std::mutex> lock(mutex_);
        running_.reset(); started_ = 0;
        completed_.notify_all();
        if (phase_ == Phase::Closed) return false;
        phase_ = pending_ ? Phase::Scheduled : Phase::Idle;
        return phase_ == Phase::Scheduled;
    }
    void close() {
        std::lock_guard<std::mutex> lock(mutex_);
        phase_ = Phase::Closed; pending_.reset(); // Running remains watched/alive.
        completed_.notify_all();
    }
    struct Stats { Phase phase; size_t running_bytes, pending_bytes; bool executing; uint64_t started, revision, execution_count; };
    Stats stats() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return {phase_, running_ ? running_->artwork.size() : 0,
            pending_ ? pending_->artwork.size() : 0, bool(running_), started_, running_ ? running_->revision : 0, execution_count_};
    }
    bool expired(uint64_t now) const {
        auto s = stats();
        return s.executing && now >= s.started && now - s.started >= NOW_PLAYING_TIMEOUT_MS;
    }
    // Closed retains Running. Scheduled must never be waited here because its
    // wake can still be in the control queue, behind this synchronous caller.
    bool wait_execution_end() {
        std::unique_lock<std::mutex> lock(mutex_);
        if (!running_) return true;
        const uint64_t now = GetTickCount64();
        const uint64_t elapsed = now >= started_ ? now - started_ : 0;
        if (elapsed >= NOW_PLAYING_TIMEOUT_MS) return false;
        return completed_.wait_for(lock, std::chrono::milliseconds(NOW_PLAYING_TIMEOUT_MS - elapsed),
            [this] { return !running_; });
    }
private:
    mutable std::mutex mutex_;
    std::condition_variable completed_;
    Phase phase_ = Phase::Idle;
    std::optional<uint64_t> watermark_;
    std::shared_ptr<const NowPlayingSnapshot> running_, pending_;
    uint64_t started_ = 0;
    uint64_t execution_count_ = 0; // Minimal history, scoped to this incarnation.
};

enum class GuiFutureWait { Ready, GuiTimeout, NowPlayingTimeout };
struct GuiFutureResult {
    GuiFutureWait outcome;
    std::shared_ptr<NowPlayingMailbox> mailbox;
};

// Same future, never a reposted command. Executing metadata owns the blockage
// and five-second deadline. After it finishes, the operation gets its normal
// budget. Ordinary GUI timeout policy is unchanged.
template<class Future, class Lookup>
GuiFutureResult await_gui_future(Future& future, std::chrono::milliseconds timeout, Lookup lookup) {
    if (future.wait_for(std::chrono::milliseconds(0)) == std::future_status::ready)
        return {GuiFutureWait::Ready, {}};
    const auto initial_mailbox = lookup();
    const auto initial_stats = initial_mailbox ? std::optional<NowPlayingMailbox::Stats>(initial_mailbox->stats()) : std::nullopt;
    auto mailbox = initial_mailbox;
    if (!initial_stats || !initial_stats->executing) {
        if (future.wait_for(timeout) == std::future_status::ready) return {GuiFutureWait::Ready, {}};
        mailbox = lookup();
        if (!mailbox) return {GuiFutureWait::GuiTimeout, {}};
        const auto current = mailbox->stats();
        if (!current.executing) {
            // A notification may have consumed the wait and already returned
            // before this timeout snapshot. Only this exact owner's execution
            // history can grant the same operation future its normal budget.
            if (mailbox != initial_mailbox || !initial_stats ||
                current.execution_count == initial_stats->execution_count)
                return {GuiFutureWait::GuiTimeout, {}};
            return {future.wait_for(timeout) == std::future_status::ready ? GuiFutureWait::Ready : GuiFutureWait::GuiTimeout, {}};
        }
    }
    if (!mailbox->wait_execution_end()) return {GuiFutureWait::NowPlayingTimeout, std::move(mailbox)};
    return {future.wait_for(timeout) == std::future_status::ready ? GuiFutureWait::Ready : GuiFutureWait::GuiTimeout, {}};
}

[[noreturn]] inline void terminate_now_playing_timeout(const std::shared_ptr<NowPlayingMailbox>& mailbox) {
    const auto stats = mailbox->stats();
    std::fprintf(stderr, "[BRIDGE] now_playing watchdog expired slot=%llu revision=%llu timeout_ms=5000; terminating host\n",
        static_cast<unsigned long long>(mailbox->slot_id), static_cast<unsigned long long>(stats.revision));
    std::fflush(stderr);
    ::ExitProcess(NOW_PLAYING_TIMEOUT_EXIT); // No generic Error or metadata ACK.
}

class NowPlayingRegistry {
public:
    uint64_t seal_all() {
        std::lock_guard<std::mutex> lock(mutex_);
        for (auto& entry : live_) entry.second->close();
        live_.clear();
        return ++epoch_;
    }
    uint64_t epoch() const { std::lock_guard<std::mutex> lock(mutex_); return epoch_; }
    void add(uint64_t slot, uint64_t epoch) {
        auto mailbox = std::make_shared<NowPlayingMailbox>(slot);
        std::lock_guard<std::mutex> lock(mutex_);
        if (epoch != epoch_) mailbox->close();
        else live_[slot] = mailbox;
        watched_.push_back(mailbox);
    }
    std::shared_ptr<NowPlayingMailbox> find(uint64_t slot) const {
        std::lock_guard<std::mutex> lock(mutex_);
        auto it = live_.find(slot);
        return it == live_.end() ? nullptr : it->second;
    }
    // Closed owners stay registered until execution finishes, even after loader
    // removal. No plugin API runs under either registry or mailbox mutex.
    std::vector<std::shared_ptr<NowPlayingMailbox>> watched() {
        std::lock_guard<std::mutex> lock(mutex_);
        watched_.erase(std::remove_if(watched_.begin(), watched_.end(), [](const auto& mailbox) {
            auto s = mailbox->stats();
            return s.phase == NowPlayingMailbox::Phase::Closed && !s.executing;
        }), watched_.end());
        return watched_;
    }
private:
    mutable std::mutex mutex_;
    uint64_t epoch_ = 0;
    std::unordered_map<uint64_t, std::shared_ptr<NowPlayingMailbox>> live_;
    std::vector<std::shared_ptr<NowPlayingMailbox>> watched_;
};

template<class Wake>
bool admit_now_playing_frame(const std::string& frame, NowPlayingRegistry& registry, Wake wake) {
    auto snapshot = std::make_shared<NowPlayingSnapshot>();
    if (!parse_now_playing(frame, *snapshot)) return false;
    auto mailbox = registry.find(snapshot->slot_id);
    if (!mailbox) return false;
    if (mailbox->admit(std::move(snapshot))) wake(mailbox);
    return true;
}

template<class Deliver, class Wake>
void execute_now_playing(const std::shared_ptr<NowPlayingMailbox>& mailbox,
                         uint64_t now, Deliver deliver, Wake wake) {
    auto snapshot = mailbox->begin(now);
    if (!snapshot) return;
    deliver(*snapshot);
    snapshot.reset();
    if (mailbox->finish()) wake(mailbox);
}

template<class Read>
bool read_control_frame(Read read, std::string& frame) {
    uint32_t length = 0;
    if (!read(&length, sizeof(length)) || length == 0 || length > MAX_CONTROL_MSG_SIZE) return false;
    frame.resize(length); // Prefix validation precedes allocation.
    return read(frame.data(), length);
}
} // namespace miv
