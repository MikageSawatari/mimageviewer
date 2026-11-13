#pragma once
#include "framing.h"
#include "asr_errors.h"
#include <limits>
#include <variant>

namespace asr_protocol {
using namespace framing;
inline constexpr uint32_t kVersion = 1;
inline constexpr uint32_t kSettingsVersion = 1;
inline constexpr size_t kMaxPcmBytes = 1024 * 1024;
enum class Kind { Hello, Load, Begin, Pcm, End, Shutdown };
inline Kind validate(const Frame & frame) {
    auto type = string_field(frame.header, "type");
    if (type == "pcm") {
        uint_field(frame.header, "request_id");
        if (frame.bytes.empty() || frame.bytes.size() % 2 || frame.bytes.size() > kMaxPcmBytes)
            throw Invalid("PCM must be 1..1 MiB of complete s16le samples");
        return Kind::Pcm;
    }
    if (!frame.bytes.empty()) throw Invalid("Unexpected binary payload");
    if (type == "hello") return Kind::Hello;
    if (type == "shutdown") return Kind::Shutdown;
    if (type == "load") {
        if (string_field(frame.header, "model_path").empty()) throw Invalid("Empty model path");
        if (uint_field(frame.header, "device_index") > std::numeric_limits<uint32_t>::max())
            throw Invalid("Vulkan device index exceeds u32");
        return Kind::Load;
    }
    if (type == "begin" || type == "end") {
        uint_field(frame.header, "request_id");
        return type == "begin" ? Kind::Begin : Kind::End;
    }
    throw Invalid("Unknown ASR request type");
}
class Session {
    enum class Idle { New, Ready, Loaded };
    struct Request { uint64_t id, samples; };
    struct Running { Request request; };
    struct Draining { Request request; std::string error; };
    std::variant<Idle, Running, Draining> state_ = Idle::New;
    bool idle(Idle expected) const {
        const auto phase = std::get_if<Idle>(&state_);
        return phase && *phase == expected;
    }
    Request & active_request() {
        if (auto running = std::get_if<Running>(&state_)) return running->request;
        if (auto draining = std::get_if<Draining>(&state_)) return draining->request;
        throw Invalid("ASR has no active input");
    }
    const Request & active_request() const {
        if (auto running = std::get_if<Running>(&state_)) return running->request;
        if (auto draining = std::get_if<Draining>(&state_)) return draining->request;
        throw Invalid("ASR has no active input");
    }
public:
    void accept(Kind kind, const json & header, size_t pcm_bytes = 0) {
        if (kind == Kind::Shutdown) return;
        if (kind == Kind::Hello && idle(Idle::New)) { state_ = Idle::Ready; return; }
        if (kind == Kind::Load && idle(Idle::Ready)) { state_ = Idle::Loaded; return; }
        if (kind == Kind::Begin && idle(Idle::Loaded)) {
            state_ = Running{{uint_field(header, "request_id"), 0}}; return;
        }
        if (streaming() && (kind == Kind::Pcm || kind == Kind::End)) {
            auto & request = active_request();
            if (uint_field(header, "request_id") != request.id) throw Invalid("ASR request ID mismatch");
            // appendが枠の途中で失敗しても、受信済みの枠全体を長さに数える。
            // Draining中も同じ受付・ID検査を通し、音声は保持せず数えるだけ。
            if (kind == Kind::Pcm) request.samples += pcm_bytes / 2;
            // EndではまだLoadedにしない。最後の窓のUTF-8検査も同じ入力に属する。
            return;
        }
        throw Invalid("ASR request out of sequence");
    }
    template<class Attempt> void run_item(Attempt && action) {
        if (draining()) return;
        if (!std::holds_alternative<Running>(state_)) throw Invalid("ASR has no running input");
        try { action(); }
        catch (const asr::InvalidUtf8 & error) {
            const auto request = std::get<Running>(state_).request;
            state_ = Draining{request, error.what()};
        }
    }
    bool streaming() const { return !std::holds_alternative<Idle>(state_); }
    bool draining() const { return std::holds_alternative<Draining>(state_); }
    json end_response() const {
        const auto & request = active_request();
        json response{{"type", "end"}, {"request_id", request.id},
            {"duration_ms", request.samples / 16}, {"status", "ok"}};
        if (const auto failed = std::get_if<Draining>(&state_)) {
            response["status"] = "item_failure";
            response["code"] = "invalid_utf8";
            response["error"] = failed->error;
        }
        return response;
    }
    void complete_end() {
        active_request();
        state_ = Idle::Loaded;
    }
    void accept_eof() const { if (streaming()) throw Invalid("EOF before ASR end"); }
};
}
