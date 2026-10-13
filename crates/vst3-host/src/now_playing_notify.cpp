#include "now_playing_notify.h"
#include "pluginterfaces/vst/ivstmessage.h"
#include <cstdio>

namespace miv {
using namespace Steinberg;
namespace {
void result_log(const NowPlayingSnapshot& s, const char* receiver, const char* operation, tresult result) {
    std::fprintf(stderr, "[BRIDGE] now_playing slot=%llu revision=%llu receiver=%s operation=%s tresult=%d image_bytes=%zu\n",
        static_cast<unsigned long long>(s.slot_id), static_cast<unsigned long long>(s.revision),
        receiver, operation, static_cast<int>(result), s.artwork.size());
    std::fflush(stderr);
}
bool fallback(tresult result) {
    return result == kNoInterface || result == kNotImplemented || result == kResultFalse;
}
template<class T>
IPtr<T> query(FUnknown* object, const NowPlayingSnapshot& s, const char* receiver, const char* operation,
              tresult* result_out = nullptr) {
    T* pointer = nullptr;
    const auto result = object ? object->queryInterface(T::iid, reinterpret_cast<void**>(&pointer)) : kNoInterface;
    if (result_out) *result_out = result;
    result_log(s, receiver, operation, result);
    if (result != kResultOk) { if (pointer) pointer->release(); return {}; }
    return owned(pointer);
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
    if (!s.artwork.empty()) {
        const auto result = attributes->setBinary("artwork", s.artwork.data(), static_cast<uint32>(s.artwork.size()));
        result_log(s, "message", "artwork", result);
        if (result != kResultOk || !set_string("artworkMimeType", s.mime)) return;
    }
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
} // namespace miv
