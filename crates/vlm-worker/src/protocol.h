#pragma once
#include "contracts.h"
#include <array>
#include <cstdint>
#include <istream>
#include <ostream>
#include <stdexcept>
#include <vector>

namespace protocol {
using json = nlohmann::json;
inline constexpr uint32_t kVersion = 1;
inline constexpr uint32_t kMaxFrame = 16 * 1024 * 1024;
struct Frame {
    json header;
    std::vector<unsigned char> bytes;
};
struct Invalid : std::runtime_error { using std::runtime_error::runtime_error; };
inline uint32_t u32(const unsigned char * p) {
    return uint32_t(p[0]) | (uint32_t(p[1]) << 8) | (uint32_t(p[2]) << 16) | (uint32_t(p[3]) << 24);
}
inline void write_u32(std::ostream & output, uint32_t value) {
    const char bytes[] = {char(value), char(value >> 8), char(value >> 16), char(value >> 24)};
    output.write(bytes, sizeof(bytes));
}
// EOF is normal only before a new outer prefix; all truncated frames are invalid.
inline bool read(std::istream & input, Frame & frame) {
    std::array<unsigned char, 4> prefix{};
    input.read(reinterpret_cast<char *>(prefix.data()), prefix.size());
    if (input.gcount() == 0 && input.eof() && !input.bad()) return false;
    if (!input) throw Invalid("Truncated outer frame length");
    auto length = u32(prefix.data());
    if (length < 4 || length > kMaxFrame) throw Invalid("Frame length outside 4..16 MiB");
    std::vector<unsigned char> body(length);
    if (!input.read(reinterpret_cast<char *>(body.data()), length)) throw Invalid("Truncated frame body");
    auto json_length = u32(body.data());
    if (!json_length || json_length > length - 4) throw Invalid("Invalid inner JSON length");
    std::string text(body.begin() + 4, body.begin() + 4 + json_length);
    // Vendored JSON lexer otherwise treats a raw NUL as end-of-input.
    if (text.find('\0') != std::string::npos || !valid_utf8(text)) throw Invalid("Invalid JSON encoding");
    frame.header = json::parse(text, nullptr, false);
    if (frame.header.is_discarded() || !frame.header.is_object()) throw Invalid("Invalid JSON object");
    frame.bytes.assign(body.begin() + 4 + json_length, body.end());
    return true;
}
inline void write(std::ostream & output, const json & header) {
    auto text = header.dump(-1, ' ', false, json::error_handler_t::replace);
    if (text.size() > kMaxFrame - 4) throw Invalid("Response exceeds frame limit");
    write_u32(output, static_cast<uint32_t>(text.size() + 4));
    write_u32(output, static_cast<uint32_t>(text.size()));
    output.write(text.data(), static_cast<std::streamsize>(text.size()));
    output.flush();
    if (!output) throw std::runtime_error("Response pipe write failed");
}
inline std::string string_field(const json & obj, const char * key) {
    if (!obj.contains(key) || !obj[key].is_string()) throw Invalid(std::string("Invalid string: ") + key);
    auto result = obj[key].get<std::string>();
    if (result.find('\0') != std::string::npos) throw Invalid(std::string("NUL in string: ") + key);
    return result;
}
inline uint64_t uint_field(const json & obj, const char * key) {
    if (!obj.contains(key) || !obj[key].is_number_unsigned()) throw Invalid(std::string("Invalid unsigned integer: ") + key);
    return obj[key].get<uint64_t>();
}
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
