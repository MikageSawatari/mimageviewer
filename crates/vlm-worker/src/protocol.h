#pragma once
#include "framing.h"

namespace protocol {
using namespace framing;
inline constexpr uint32_t kVersion = 2;
enum class Kind { Hello, Load, Caption, Shutdown };
inline Kind validate(const Frame & frame) {
    auto type = string_field(frame.header, "type");
    if (type == "caption") {
        uint_field(frame.header, "request_id");
        auto width = uint_field(frame.header, "width");
        auto height = uint_field(frame.header, "height");
        if (!width || !height || width > 1024 || height > 1024 || frame.bytes.size() != width * height * 3)
            throw Invalid("Caption dimensions or RGB byte count invalid");
        return Kind::Caption;
    }
    if (!frame.bytes.empty()) throw Invalid("Unexpected binary payload");
    if (type == "hello") return Kind::Hello;
    if (type == "shutdown") return Kind::Shutdown;
    if (type != "load") throw Invalid("Unknown request type");
    if (string_field(frame.header, "model_path").empty() || string_field(frame.header, "mmproj_path").empty())
        throw Invalid("Empty model path");
    auto luid = string_field(frame.header, "device_luid");
    if (!luid.empty()) {
        if (luid.size() != 17 || luid[8] != ':') throw Invalid("Invalid device LUID");
        for (size_t i = 0; i < luid.size(); ++i) if (i != 8 && !((luid[i] >= '0' && luid[i] <= '9')
            || (luid[i] >= 'a' && luid[i] <= 'f') || (luid[i] >= 'A' && luid[i] <= 'F'))) throw Invalid("Invalid device LUID");
    }
    auto threads = uint_field(frame.header, "cpu_threads");
    if (!threads || threads > 1024) throw Invalid("CPU threads outside 1..1024");
    return Kind::Load;
}
// One handshake, one immutable model load, then serial captions. Shutdown is
// allowed before load, so a caller can leave after inspecting worker identity.
class Session {
    enum class State { New, Ready, Loaded } state_ = State::New;
public:
    void accept(Kind kind) {
        if (kind == Kind::Hello && state_ == State::New) { state_ = State::Ready; return; }
        if (kind == Kind::Load && state_ == State::Ready) { state_ = State::Loaded; return; }
        if ((kind == Kind::Caption && state_ == State::Loaded) || kind == Kind::Shutdown) return;
        throw Invalid("Request out of sequence");
    }
};
}
