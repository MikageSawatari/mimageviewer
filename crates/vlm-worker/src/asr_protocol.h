#pragma once
#include "framing.h"
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
        if (frame.header.contains("device_index")) throw Invalid("ASR load requires device_luid, not device_index");
        const auto luid = string_field(frame.header, "device_luid");
        if (luid.size() != 17 || luid[8] != ':') throw Invalid("Invalid ASR device LUID");
        for (size_t i = 0; i < luid.size(); ++i) if (i != 8 && !((luid[i] >= '0' && luid[i] <= '9') ||
            (luid[i] >= 'a' && luid[i] <= 'f') || (luid[i] >= 'A' && luid[i] <= 'F'))) throw Invalid("Invalid ASR device LUID");
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
    struct Request { uint64_t id, samples, discarded_bytes, discarded_fragments; };
    struct Running { Request request; };
    std::variant<Idle, Running> state_ = Idle::New;
    bool idle(Idle expected) const {
        const auto phase = std::get_if<Idle>(&state_);
        return phase && *phase == expected;
    }
    Request & active_request() {
        if (auto running = std::get_if<Running>(&state_)) return running->request;
        throw Invalid("ASR has no active input");
    }
    const Request & active_request() const {
        if (auto running = std::get_if<Running>(&state_)) return running->request;
        throw Invalid("ASR has no active input");
    }
public:
    void accept(Kind kind, const json & header, size_t pcm_bytes = 0) {
        if (kind == Kind::Shutdown) return;
        if (kind == Kind::Hello && idle(Idle::New)) { state_ = Idle::Ready; return; }
        if (kind == Kind::Load && idle(Idle::Ready)) { state_ = Idle::Loaded; return; }
        if (kind == Kind::Begin && idle(Idle::Loaded)) {
            state_ = Running{{uint_field(header, "request_id"), 0, 0, 0}}; return;
        }
        if (streaming() && (kind == Kind::Pcm || kind == Kind::End)) {
            auto & request = active_request();
            if (uint_field(header, "request_id") != request.id) throw Invalid("ASR request ID mismatch");
            // 受信済みの枠全体を長さに数える。抽出失敗時の早いendも同じ境界で終える。
            if (kind == Kind::Pcm) request.samples += pcm_bytes / 2;
            // EndではまだLoadedにしない。最後の窓のUTF-8検査も同じ入力に属する。
            return;
        }
        throw Invalid("ASR request out of sequence");
    }
    bool streaming() const { return !std::holds_alternative<Idle>(state_); }
    void record_window(uint64_t discarded_bytes, uint64_t discarded_fragments) {
        auto & request = active_request();
        request.discarded_bytes += discarded_bytes;
        request.discarded_fragments += discarded_fragments;
    }
    json end_response() const {
        const auto & request = active_request();
        json response{{"type", "end"}, {"request_id", request.id},
            {"duration_ms", request.samples / 16}, {"status", "ok"},
            {"utf8_discarded_bytes", request.discarded_bytes}, {"utf8_discarded_fragments", request.discarded_fragments}};
        return response;
    }
    void complete_end() {
        active_request();
        state_ = Idle::Loaded;
    }
    void accept_eof() const { if (streaming()) throw Invalid("EOF before ASR end"); }
};
}
