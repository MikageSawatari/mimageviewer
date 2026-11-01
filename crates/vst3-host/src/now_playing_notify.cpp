#include "now_playing_notify.h"
#include "pluginterfaces/vst/ivstmessage.h"
#include <cstdio>
#include <cstring>

namespace miv {
using namespace Steinberg;
namespace {
void result_log(const NowPlayingSnapshot& s, const char* receiver, const char* operation, tresult result) {
    std::fprintf(stderr, "[BRIDGE] now_playing slot=%llu revision=%llu receiver=%s operation=%s tresult=%d image_bytes=%zu\n",
        static_cast<unsigned long long>(s.slot_id), static_cast<unsigned long long>(s.revision),
        receiver, operation, static_cast<int>(result), s.artwork.size());
    std::fflush(stderr);
}
void result_log(const PlaybackPositionSnapshot& s, const char* receiver, const char* operation, tresult result) {
    std::fprintf(stderr, "[BRIDGE] playback_position slot=%llu generation=%llu receiver=%s operation=%s tresult=%d\n",
        static_cast<unsigned long long>(s.slot_id), static_cast<unsigned long long>(s.media_generation),
        receiver, operation, static_cast<int>(result));
    std::fflush(stderr);
}
// VST3 attributes are signed; preserve the full wire u64 bit pattern.
int64 attribute_int(uint64_t value) {
    int64 result;
    static_assert(sizeof(result) == sizeof(value));
    std::memcpy(&result, &value, sizeof(result));
    return result;
}
bool fallback(tresult result) {
    return result == kNoInterface || result == kNotImplemented || result == kResultFalse;
}
template<class T, class Snapshot>
IPtr<T> query(FUnknown* object, const Snapshot& s, const char* receiver, const char* operation,
              tresult* result_out = nullptr) {
    T* pointer = nullptr;
    const auto result = object ? object->queryInterface(T::iid, reinterpret_cast<void**>(&pointer)) : kNoInterface;
    if (result_out) *result_out = result;
    result_log(s, receiver, operation, result);
    if (result != kResultOk) { if (pointer) pointer->release(); return {}; }
    return owned(pointer);
}
template<class Snapshot>
void deliver_message(Vst::IMessage* message, FUnknown* component, FUnknown* controller, const Snapshot& s) {
    tresult component_query = kNoInterface;
    auto component_cp = query<Vst::IConnectionPoint>(component, s, "component", "query", &component_query);
    if (!component_cp && !fallback(component_query)) return;
    if (component_cp) {
        const auto result = component_cp->notify(message);
        result_log(s, "component", "notify", result);
        if (!fallback(result)) return;
    }
    auto controller_cp = query<Vst::IConnectionPoint>(controller, s, "controller", "query");
    if (!controller_cp) return;
    auto component_identity = component_cp ? query<FUnknown>(component_cp.get(), s, "component", "identity") : IPtr<FUnknown>{};
    auto controller_identity = component_cp ? query<FUnknown>(controller_cp.get(), s, "controller", "identity") : IPtr<FUnknown>{};
    if (component_cp && (!component_identity || !controller_identity || component_identity == controller_identity)) return;
    const auto result = controller_cp->notify(message);
    result_log(s, "controller", "notify", result);
}
}
void notify_now_playing(Vst::IHostApplication* host, FUnknown* component, FUnknown* controller,
                        const NowPlayingSnapshot& s) {
    auto message = owned(Vst::allocateMessage(host));
    if (!message) { result_log(s, "message", "allocate", kOutOfMemory); return; }
    result_log(s, "message", "allocate", kResultOk);
    message->setMessageID("EffeTune.NowPlaying");
    auto* attributes = message->getAttributes();
    if (!attributes) { result_log(s, "message", "attributes", kOutOfMemory); return; }
    const auto set_string = [&](const char* key, const std::wstring& value) {
        const auto result = attributes->setString(key, reinterpret_cast<const Vst::TChar*>(value.c_str()));
        result_log(s, "message", key, result);
        return result == kResultOk;
    };
    if (!set_string("title", s.title) || !set_string("album", s.album) || !set_string("artist", s.artist)) return;
    if (s.media_generation) {
        const auto result = attributes->setInt("mediaGeneration", attribute_int(*s.media_generation));
        result_log(s, "message", "mediaGeneration", result);
        if (result != kResultOk) return;
    }
    if (s.lyrics_sidecar && !set_string("lyricsSidecar", *s.lyrics_sidecar)) return;
    if (s.lyrics_embedded && !set_string("lyricsEmbedded", *s.lyrics_embedded)) return;
    if (s.duration_seconds) {
        const auto result = attributes->setFloat("durationSeconds", *s.duration_seconds);
        result_log(s, "message", "durationSeconds", result);
        if (result != kResultOk) return;
    }
    if (!s.artwork.empty()) {
        const auto result = attributes->setBinary("artwork", s.artwork.data(), static_cast<uint32>(s.artwork.size()));
        result_log(s, "message", "artwork", result);
        if (result != kResultOk || !set_string("artworkMimeType", s.mime)) return;
    }
    deliver_message(message, component, controller, s);
}
void notify_playback_position(Vst::IHostApplication* host, FUnknown* component, FUnknown* controller,
                              const PlaybackPositionSnapshot& s) {
    auto message = owned(Vst::allocateMessage(host));
    if (!message) { result_log(s, "message", "allocate", kOutOfMemory); return; }
    message->setMessageID("EffeTune.PlaybackPosition");
    auto* attributes = message->getAttributes();
    if (!attributes) { result_log(s, "message", "attributes", kOutOfMemory); return; }
    const auto set_int = [&](const char* key, uint64_t value) {
        const auto result = attributes->setInt(key, attribute_int(value));
        result_log(s, "message", key, result);
        return result == kResultOk;
    };
    const auto set_float = [&](const char* key, double value) {
        const auto result = attributes->setFloat(key, value);
        result_log(s, "message", key, result);
        return result == kResultOk;
    };
    if (!set_int("mediaGeneration", s.media_generation) || !set_int("available", s.available) ||
        !set_int("playing", s.playing) || !set_float("positionSeconds", s.position_seconds) ||
        !set_float("rate", s.rate) || !set_int("hostTimeNanoseconds", s.host_time_ns)) return;
    deliver_message(message, component, controller, s);
}
} // namespace miv
