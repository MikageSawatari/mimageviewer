#pragma once

#include "protocol.h"
#include "ipc_strings.h"
#include "utf8_paths.h"
#include <chrono>
#include <cmath>
#include <cstdlib>
#include <condition_variable>
#include <cstdio>
#include <future>
#include <functional>
#include <memory>
#include <mutex>
#include <optional>
#include <string_view>
#include <type_traits>
#include <unordered_map>
#include <vector>

namespace miv {
constexpr size_t NOW_PLAYING_IMAGE_MAX = 8388608;
constexpr size_t NOW_PLAYING_WIRE_MAX = 11184812;
constexpr size_t NOW_PLAYING_LYRICS_MAX = 262144;
constexpr uint64_t NOW_PLAYING_TIMEOUT_MS = 5000;
constexpr DWORD NOW_PLAYING_TIMEOUT_EXIT = 0xEFFEC002u;

struct NowPlayingSnapshot {
    uint64_t slot_id = 0, revision = 0;
    std::wstring title, album, artist, mime;
    std::vector<uint8_t> artwork;
    std::optional<uint64_t> media_generation;
    std::optional<std::wstring> lyrics_sidecar, lyrics_embedded;
    std::optional<double> duration_seconds;
};

struct PlaybackPositionSnapshot {
    uint64_t slot_id = 0, media_generation = 0, host_time_ns = 0;
    bool available = false, playing = false;
    double position_seconds = 0, rate = 1;
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
inline bool now_playing_text(const std::string& json, size_t& p, std::wstring& out, size_t limit = 4096) {
    std::string text;
    if (!json_string(json, p, text)) return false;
    try { out = utf8_to_utf16(text); } catch (const std::exception&) { return false; }
    if (out.size() > limit) {
        out.resize(limit);
        if (out.back() >= 0xd800 && out.back() <= 0xdbff) out.pop_back();
    }
    return true;
}
inline bool now_playing_number(const std::string& json, size_t& p, double& value) {
    const size_t start = p;
    if (p < json.size() && json[p] == '-') ++p;
    // Parse grammar independently of integer range (large finite doubles are valid).
    const size_t digits = p;
    while (p < json.size() && json[p] >= '0' && json[p] <= '9') ++p;
    if (p == digits || (p - digits > 1 && json[digits] == '0')) return false;
    if (p < json.size() && json[p] == '.') {
        const size_t fraction = ++p;
        while (p < json.size() && json[p] >= '0' && json[p] <= '9') ++p;
        if (p == fraction) return false;
    }
    if (p < json.size() && (json[p] == 'e' || json[p] == 'E')) {
        ++p;
        if (p < json.size() && (json[p] == '+' || json[p] == '-')) ++p;
        const size_t exponent = p;
        while (p < json.size() && json[p] >= '0' && json[p] <= '9') ++p;
        if (p == exponent) return false;
    }
    const std::string token = json.substr(start, p - start);
    char* end = nullptr;
    value = std::strtod(token.c_str(), &end);
    return end == token.c_str() + token.size() && std::isfinite(value);
}
template<class T, class Read>
bool now_playing_optional(const std::string& json, size_t& p, std::optional<T>& result, Read read) {
    if (json.compare(p, 4, "null") == 0) { p += 4; result.reset(); return true; }
    T value{};
    if (!read(value)) return false;
    result = std::move(value);
    return true;
}
inline bool parse_now_playing(const std::string& json, NowPlayingSnapshot& result) {
    if (json.size() > MAX_CONTROL_MSG_SIZE) return false;
    size_t p = 0;
    unsigned seen = 0;
    std::string_view art;
    result = {};
    now_playing_space(json, p);
    if (p == json.size() || json[p++] != '{') return false;
    for (unsigned field = 0; field < 12; ++field) {
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
        else if (name == "artworkMimeType") { bit = 128; ok = now_playing_text(json, p, result.mime, MAX_CONTROL_MSG_SIZE); }
        else if (name == "mediaGeneration") { bit = 256; ok = now_playing_optional(json, p, result.media_generation,
            [&](auto& v) { return now_playing_uint(json, p, v); }); }
        else if (name == "lyricsSidecar" || name == "lyricsEmbedded") {
            bit = name == "lyricsSidecar" ? 512 : 1024;
            auto& lyrics = name == "lyricsSidecar" ? result.lyrics_sidecar : result.lyrics_embedded;
            ok = now_playing_optional(json, p, lyrics, [&](auto& v) { return now_playing_text(json, p, v, NOW_PLAYING_LYRICS_MAX); });
        }
        else if (name == "durationSeconds") { bit = 2048; ok = now_playing_optional(json, p, result.duration_seconds,
            [&](auto& v) { return now_playing_number(json, p, v) && v >= 0; }); }
        if (!ok || (seen & bit)) return false;
        seen |= bit;
        now_playing_space(json, p);
        if (p == json.size()) return false;
        if (json[p] == '}') { ++p; break; }
        if (field == 11) return false;
        if (json[p++] != ',') return false;
    }
    now_playing_space(json, p);
    if (p != json.size() || (seen & 255) != 255 || !now_playing_decode_artwork(art, result.artwork)) return false;
    return result.artwork.empty() ? result.mime.empty() :
        result.mime == L"image/png" || result.mime == L"image/jpeg" ||
        result.mime == L"image/gif" || result.mime == L"image/webp";
}

inline bool parse_playback_position(const std::string& json, PlaybackPositionSnapshot& result) {
    if (json.size() > MAX_CONTROL_MSG_SIZE) return false;
    result = {};
    size_t p = 0;
    unsigned seen = 0;
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
        if (name == "cmd" && field == 0) { std::string cmd; bit = 1; ok = json_string(json, p, cmd) && cmd == "set_playback_position"; }
        else if (name == "slot_id") { bit = 2; ok = now_playing_uint(json, p, result.slot_id); }
        else if (name == "mediaGeneration") { bit = 4; ok = now_playing_uint(json, p, result.media_generation); }
        else if (name == "available" || name == "playing") {
            bit = name == "available" ? 8 : 16;
            bool& value = name == "available" ? result.available : result.playing;
            uint64_t integer = 0;
            ok = now_playing_uint(json, p, integer) && integer <= 1;
            value = integer != 0;
        }
        else if (name == "positionSeconds") { bit = 32; ok = now_playing_number(json, p, result.position_seconds) && result.position_seconds >= 0; }
        else if (name == "rate") { bit = 64; ok = now_playing_number(json, p, result.rate) && result.rate > 0; }
        else if (name == "hostTimeNanoseconds") { bit = 128; ok = now_playing_uint(json, p, result.host_time_ns); }
        if (!ok || (seen & bit)) return false;
        seen |= bit;
        now_playing_space(json, p);
        if (p == json.size() || json[p++] != (field == 7 ? '}' : ',')) return false;
    }
    now_playing_space(json, p);
    return p == json.size() && seen == 255;
}

// The single owner spans reader, control and GUI. Scheduled means exactly one
// token across the two queues; Running can retain exactly one latest pending.
class NotificationMailbox {
public:
    enum class Phase { Idle, Scheduled, Running, Closed };
    struct Stats { Phase phase; size_t running_bytes, pending_bytes; bool executing; uint64_t started, revision, execution_count; };
    explicit NotificationMailbox(uint64_t id) : slot_id(id) {}
    virtual ~NotificationMailbox() = default;
    const uint64_t slot_id;
    virtual Stats stats() const = 0;
    virtual bool wait_execution_end() = 0;
    virtual const char* label() const = 0;
    bool expired(uint64_t now) const {
        auto s = stats();
        return s.executing && now >= s.started && now - s.started >= NOW_PLAYING_TIMEOUT_MS;
    }
};

template<class Snapshot>
class LatestNotificationMailbox : public NotificationMailbox {
public:
    explicit LatestNotificationMailbox(uint64_t id) : NotificationMailbox(id) {}
    const char* label() const override {
        if constexpr (std::is_same_v<Snapshot, NowPlayingSnapshot>) return "now_playing";
        else return "playback_position";
    }
    bool admit(std::shared_ptr<const Snapshot> snapshot) {
        std::lock_guard<std::mutex> lock(mutex_);
        if (phase_ == Phase::Closed) return false;
        if constexpr (std::is_same_v<Snapshot, NowPlayingSnapshot>) {
            if (watermark_ && snapshot->revision <= *watermark_) return false;
            watermark_ = snapshot->revision;
        }
        pending_ = std::move(snapshot);
        if (phase_ != Phase::Idle) return false;
        phase_ = Phase::Scheduled;
        return true;
    }
    std::shared_ptr<const Snapshot> begin(uint64_t now) {
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
    Stats stats() const override {
        std::lock_guard<std::mutex> lock(mutex_);
        if constexpr (std::is_same_v<Snapshot, NowPlayingSnapshot>) {
            return {phase_, running_ ? running_->artwork.size() : 0,
                pending_ ? pending_->artwork.size() : 0, bool(running_), started_, running_ ? running_->revision : 0, execution_count_};
        } else return {phase_, 0, 0, bool(running_), started_, running_ ? running_->media_generation : 0, execution_count_};
    }
    // Closed retains Running. Scheduled must never be waited here because its
    // wake can still be in the control queue, behind this synchronous caller.
    bool wait_execution_end() override {
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
    std::shared_ptr<const Snapshot> running_, pending_;
    uint64_t started_ = 0;
    uint64_t execution_count_ = 0; // Minimal history, scoped to this incarnation.
};
using NowPlayingMailbox = LatestNotificationMailbox<NowPlayingSnapshot>;
using PlaybackPositionMailbox = LatestNotificationMailbox<PlaybackPositionSnapshot>;

enum class GuiFutureWait { Ready, GuiTimeout, NowPlayingTimeout };
struct GuiFutureResult {
    GuiFutureWait outcome;
    std::shared_ptr<NotificationMailbox> mailbox;
};

// A slot has two independent latest-value lanes, but a single serial GUI queue.
// Account for either lane's exact incarnation/history when a synchronous GUI
// operation waits behind notify. A Scheduled lane is never waited directly.
template<class Future, class Lookup>
GuiFutureResult await_gui_notifications(Future& future, std::chrono::milliseconds timeout, Lookup lookup) {
    if (future.wait_for(std::chrono::milliseconds(0)) == std::future_status::ready)
        return {GuiFutureWait::Ready, {}};
    const auto initial = lookup();
    std::vector<uint64_t> executions;
    for (const auto& owner : initial) executions.push_back(owner->stats().execution_count);
    bool notify_ran = false;
    for (size_t i = 0; i < initial.size(); ++i) {
        const auto& owner = initial[i];
        if (!owner->stats().executing) continue;
        if (!owner->wait_execution_end()) return {GuiFutureWait::NowPlayingTimeout, owner};
        executions[i] = owner->stats().execution_count;
    }
    for (;;) {
        if (future.wait_for(timeout) == std::future_status::ready) return {GuiFutureWait::Ready, {}};
        bool waited = false;
        for (const auto& owner : lookup()) {
            const auto current = owner->stats();
            if (current.executing) {
                waited = true;
                if (!owner->wait_execution_end()) return {GuiFutureWait::NowPlayingTimeout, owner};
            }
            for (size_t i = 0; i < initial.size(); ++i) {
                if (owner == initial[i] && current.execution_count != executions[i]) {
                    notify_ran = true;
                    executions[i] = current.execution_count;
                }
            }
        }
        if (!waited && !notify_ran) return {GuiFutureWait::GuiTimeout, {}};
        notify_ran = false; // Grant the same operation its ordinary budget once.
    }
}

template<class Future, class Lookup>
GuiFutureResult await_gui_future(Future& future, std::chrono::milliseconds timeout, Lookup lookup) {
    return await_gui_notifications(future, timeout, [&] {
        std::vector<std::shared_ptr<NotificationMailbox>> owners;
        if (auto owner = lookup()) owners.push_back(std::move(owner));
        return owners;
    });
}

[[noreturn]] inline void terminate_now_playing_timeout(const std::shared_ptr<NotificationMailbox>& mailbox) {
    const auto stats = mailbox->stats();
    std::fprintf(stderr, "[BRIDGE] %s watchdog expired slot=%llu revision=%llu timeout_ms=5000; terminating host\n",
        mailbox->label(), static_cast<unsigned long long>(mailbox->slot_id), static_cast<unsigned long long>(stats.revision));
    std::fflush(stderr);
    ::ExitProcess(NOW_PLAYING_TIMEOUT_EXIT); // No generic Error or metadata ACK.
}

template<class Mailbox>
class NotificationRegistry {
public:
    uint64_t seal_all() {
        std::lock_guard<std::mutex> lock(mutex_);
        for (auto& entry : live_) entry.second->close();
        live_.clear();
        return ++epoch_;
    }
    uint64_t epoch() const { std::lock_guard<std::mutex> lock(mutex_); return epoch_; }
    void add(uint64_t slot, uint64_t epoch) {
        auto mailbox = std::make_shared<Mailbox>(slot);
        std::lock_guard<std::mutex> lock(mutex_);
        if (epoch != epoch_) mailbox->close();
        else live_[slot] = mailbox;
        watched_.push_back(mailbox);
    }
    std::shared_ptr<Mailbox> find(uint64_t slot) const {
        std::lock_guard<std::mutex> lock(mutex_);
        auto it = live_.find(slot);
        return it == live_.end() ? nullptr : it->second;
    }
    // Closed owners stay registered until execution finishes, even after loader
    // removal. No plugin API runs under either registry or mailbox mutex.
    std::vector<std::shared_ptr<Mailbox>> watched() {
        std::lock_guard<std::mutex> lock(mutex_);
        watched_.erase(std::remove_if(watched_.begin(), watched_.end(), [](const auto& mailbox) {
            auto s = mailbox->stats();
            return s.phase == NotificationMailbox::Phase::Closed && !s.executing;
        }), watched_.end());
        return watched_;
    }
private:
    mutable std::mutex mutex_;
    uint64_t epoch_ = 0;
    std::unordered_map<uint64_t, std::shared_ptr<Mailbox>> live_;
    std::vector<std::shared_ptr<Mailbox>> watched_;
};
using NowPlayingRegistry = NotificationRegistry<NowPlayingMailbox>;
using PlaybackPositionRegistry = NotificationRegistry<PlaybackPositionMailbox>;

template<class Wake>
bool admit_now_playing_frame(const std::string& frame, NowPlayingRegistry& registry, Wake wake) {
    auto snapshot = std::make_shared<NowPlayingSnapshot>();
    if (!parse_now_playing(frame, *snapshot)) return false;
    auto mailbox = registry.find(snapshot->slot_id);
    if (!mailbox) return false;
    if (mailbox->admit(std::move(snapshot))) wake(mailbox);
    return true;
}

template<class Mailbox, class Deliver, class Wake>
void execute_now_playing(const std::shared_ptr<Mailbox>& mailbox,
                         uint64_t now, Deliver deliver, Wake wake) {
    auto snapshot = mailbox->begin(now);
    if (!snapshot) return;
    deliver(*snapshot);
    snapshot.reset();
    if (mailbox->finish()) wake(mailbox);
}

template<class Wake>
bool admit_playback_position_frame(const std::string& frame, PlaybackPositionRegistry& registry, Wake wake) {
    auto snapshot = std::make_shared<PlaybackPositionSnapshot>();
    if (!parse_playback_position(frame, *snapshot)) return false;
    auto mailbox = registry.find(snapshot->slot_id);
    if (!mailbox) return false;
    if (mailbox->admit(std::move(snapshot))) wake(mailbox);
    return true;
}

template<class Read>
bool read_control_frame(Read read, std::string& frame) {
    uint32_t length = 0;
    if (!read(&length, sizeof(length)) || length == 0 || length > MAX_CONTROL_MSG_SIZE) return false;
    frame.resize(length); // Prefix validation precedes allocation.
    return read(frame.data(), length);
}
} // namespace miv
