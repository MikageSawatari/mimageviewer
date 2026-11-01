#include "now_playing_notify.h"
#include "public.sdk/source/vst/hosting/hostclasses.h"
#include <atomic>
#include <condition_variable>
#include <cstdio>
#include <cstring>
#include <deque>
#include <iostream>
#include <thread>
#include <fcntl.h>
#include <io.h>

using namespace Steinberg;
using namespace Steinberg::Vst;
using namespace miv;
static void require(bool value, const char* label) {
    if (!value) throw std::runtime_error(label); // Release checks must survive NDEBUG.
}

// Inert SDK objects only. No plugin module, editor, HWND or user profile.
class Receiver : public IConnectionPoint {
public:
    std::atomic<int> calls{0};
    tresult result = kResultOk;
    bool supported = true;
    FUnknown* identity = nullptr;
    DWORD thread = 0;
    IPtr<IMessage> held;
    const char* expected_id = "EffeTune.NowPlaying";
    std::function<void()> entered;
    tresult PLUGIN_API queryInterface(const TUID interface_id, void** obj) override {
        if (identity && FUnknownPrivate::iidEqual(interface_id, FUnknown::iid)) {
            *obj = identity; identity->addRef(); return kResultOk;
        }
        if (FUnknownPrivate::iidEqual(interface_id, FUnknown::iid) ||
            (supported && FUnknownPrivate::iidEqual(interface_id, IConnectionPoint::iid))) {
            *obj = static_cast<IConnectionPoint*>(this); addRef(); return kResultOk;
        }
        *obj = nullptr; return kNoInterface;
    }
    uint32 PLUGIN_API addRef() override { return ++refs_; }
    uint32 PLUGIN_API release() override { return --refs_; } // Stack fixture.
    tresult PLUGIN_API connect(IConnectionPoint*) override { return kResultOk; }
    tresult PLUGIN_API disconnect(IConnectionPoint*) override { return kResultOk; }
    tresult PLUGIN_API notify(IMessage* message) override {
        ++calls; thread = GetCurrentThreadId(); held = message;
        require(std::strcmp(message->getMessageID(), expected_id) == 0, "message ID");
        if (entered) entered();
        return result;
    }
private:
    std::atomic<uint32> refs_{1};
};

class FailingAttributes : public IAttributeList {
public:
    explicit FailingAttributes(std::string key) : key_(std::move(key)), actual_(HostAttributeList::make()) {}
    tresult PLUGIN_API queryInterface(const TUID, void** obj) override { *obj = nullptr; return kNoInterface; }
    uint32 PLUGIN_API addRef() override { return ++refs_; }
    uint32 PLUGIN_API release() override { const auto n = --refs_; if (!n) delete this; return n; }
    tresult PLUGIN_API setInt(AttrID id, int64 value) override { return key_ == id ? kInvalidArgument : actual_->setInt(id, value); }
    tresult PLUGIN_API getInt(AttrID id, int64& value) override { return actual_->getInt(id, value); }
    tresult PLUGIN_API setFloat(AttrID id, double value) override { return key_ == id ? kInvalidArgument : actual_->setFloat(id, value); }
    tresult PLUGIN_API getFloat(AttrID id, double& value) override { return actual_->getFloat(id, value); }
    tresult PLUGIN_API setString(AttrID id, const TChar* value) override { return key_ == id ? kInvalidArgument : actual_->setString(id, value); }
    tresult PLUGIN_API getString(AttrID id, TChar* value, uint32 bytes) override { return actual_->getString(id, value, bytes); }
    tresult PLUGIN_API setBinary(AttrID id, const void* data, uint32 bytes) override { return key_ == id ? kInvalidArgument : actual_->setBinary(id, data, bytes); }
    tresult PLUGIN_API getBinary(AttrID id, const void*& data, uint32& bytes) override { return actual_->getBinary(id, data, bytes); }
private:
    std::string key_;
    IPtr<IAttributeList> actual_;
    std::atomic<uint32> refs_{1};
};
class FailingMessage : public IMessage {
public:
    explicit FailingMessage(std::string key) : attributes_(owned(new FailingAttributes(std::move(key)))) {}
    tresult PLUGIN_API queryInterface(const TUID, void** obj) override { *obj = nullptr; return kNoInterface; }
    uint32 PLUGIN_API addRef() override { return ++refs_; }
    uint32 PLUGIN_API release() override { const auto n = --refs_; if (!n) delete this; return n; }
    const char* PLUGIN_API getMessageID() override { return id_.c_str(); }
    void PLUGIN_API setMessageID(const char* id) override { id_ = id; }
    IAttributeList* PLUGIN_API getAttributes() override { return attributes_; }
private:
    std::string id_;
    IPtr<IAttributeList> attributes_;
    std::atomic<uint32> refs_{1};
};
class FailingHost : public Steinberg::Vst::HostApplication {
public:
    std::string key;
    tresult PLUGIN_API createInstance(TUID, TUID, void** obj) override {
        if (key == "allocate") { *obj = nullptr; return kOutOfMemory; }
        *obj = new FailingMessage(key); return kResultOk;
    }
};

static std::string frame(uint64_t revision, const std::string& title = "", const std::string& art = "", const std::string& mime = "", uint64_t slot = 0) {
    return "{\"cmd\":\"set_now_playing\",\"slot_id\":" + std::to_string(slot) +
        ",\"revision\":" + std::to_string(revision) + ",\"title\":\"" + title +
        "\",\"album\":\"\",\"artist\":\"\",\"artwork\":\"" + art + "\",\"artworkMimeType\":\"" + mime + "\"}";
}
static std::string maximum_art() { return std::string(NOW_PLAYING_WIRE_MAX - 1, 'A') + '='; }
static void parser_tests() {
    NowPlayingSnapshot snapshot;
    require(parse_now_playing(frame(1, "\\\"cmd\\\":\\\"close\\\",\\\"slot_id\\\":99\\\\ 日本語\\ud83d\\ude00"), snapshot), "key-looking title");
    require(snapshot.slot_id == 0 && snapshot.revision == 1 && snapshot.title.find(L"日本語") != std::wstring::npos, "top-level fields");
    for (size_t units : {4095, 4096, 4097}) {
        require(parse_now_playing(frame(2, std::string(units, 'a')), snapshot), "long text accepted");
        require(snapshot.title.size() == std::min(size_t(4096), units), "4096 UTF-16 truncation");
    }
    require(parse_now_playing(frame(3, std::string(4095, 'a') + "\\ud83d\\ude00"), snapshot) && snapshot.title.size() == 4095, "emoji truncate boundary");
    require(parse_now_playing(frame(3, std::string(4094, 'a') + "\\ud83d\\ude00"), snapshot) && snapshot.title.size() == 4096, "emoji exact boundary");
    for (const auto& title : {std::string("\\u0000"), std::string("\\ud800"), std::string("\\udfff"), std::string("\xc0\xaf"), std::string("\xed\xa0\x80")})
        require(!parse_now_playing(frame(4, title), snapshot), "strict Unicode rejection");
    auto max = maximum_art();
    auto max_frame = frame(5, "", max, "image/png");
    require(max_frame.size() < MAX_CONTROL_MSG_SIZE && parse_now_playing(max_frame, snapshot), "maximum image frame");
    require(snapshot.artwork.size() == NOW_PLAYING_IMAGE_MAX, "exact decoded limit");
    require(!parse_now_playing(frame(6, "", std::string(NOW_PLAYING_WIRE_MAX, 'A'), "image/png"), snapshot), "decoded +1 same wire size");
    require(!parse_now_playing(frame(6, "", max + "A", "image/png"), snapshot), "wire +1");
    for (const auto& bad : {"AA=", "AA===", "AA=A", "AB==", "AAF=", "AAAA\n", "____", "A===", "===="})
        require(!parse_now_playing(frame(6, "", bad, "image/png"), snapshot), "malformed base64");
    for (const auto& mime : {"image/png", "image/jpeg", "image/gif", "image/webp"})
        require(parse_now_playing(frame(7, "", "AA==", mime), snapshot), "supported MIME");
    require(!parse_now_playing(frame(7, "", "AA==", "text/plain"), snapshot), "MIME rejected");
    require(!parse_now_playing(frame(7, "", "", "image/png"), snapshot), "empty image MIME mismatch");
    require(!parse_now_playing(frame(7, "", "AA==", ""), snapshot), "missing MIME");
    require(!parse_now_playing("{\"cmd\":\"set_now_playing\",\"slot_id\":false}", snapshot), "wrong type");
    auto duplicate = frame(7); duplicate.replace(duplicate.find("\"album\""), 7, "\"title\"");
    require(!parse_now_playing(duplicate, snapshot), "duplicate field");
    require(!parse_now_playing("\v" + frame(7), snapshot), "strict JSON whitespace");
    auto overflow = frame(7); overflow.replace(overflow.find("\"slot_id\":0"), 11, "\"slot_id\":18446744073709551616");
    require(!parse_now_playing(overflow, snapshot), "integer overflow");
    std::string framed;
    size_t reads = 0;
    require(read_control_frame([&](void* data, size_t length) {
        ++reads; if (reads == 1) { const uint32_t n = MAX_CONTROL_MSG_SIZE; std::memcpy(data, &n, 4); }
        else std::memset(data, ' ', length); return true;
    }, framed) && framed.size() == MAX_CONTROL_MSG_SIZE && reads == 2, "frame max accepted");
    framed.clear(); reads = 0;
    require(!read_control_frame([&](void* data, size_t) {
        ++reads; const uint32_t n = MAX_CONTROL_MSG_SIZE + 1; std::memcpy(data, &n, 4); return true;
    }, framed) && framed.empty() && reads == 1, "frame +1 rejected before allocation");
}
static void receiver_tests() {
    auto host = owned(new Steinberg::Vst::HostApplication);
    NowPlayingSnapshot s; s.slot_id = 0; s.revision = 1;
    s.title = L"日本語 \"title\" 😀"; s.artwork = {1, 2, 3}; s.mime = L"image/png";
    Receiver component, controller;
    const auto send = [&] { notify_now_playing(host, &component, &controller, s); };
    send(); require(component.calls == 1 && controller.calls == 0, "component acceptance once");
    require(component.thread == GetCurrentThreadId(), "receiver GUI thread identity");
    notify_now_playing(host, &component, &component, s);
    require(component.calls == 2, "same identity only once");
    component.result = kResultFalse;
    notify_now_playing(host, &component, &component, s);
    require(component.calls == 3, "same identity no fallback repeat");
    Receiver wrapper;
    wrapper.identity = &component;
    notify_now_playing(host, &component, &wrapper, s);
    require(wrapper.calls == 0, "distinct interface pointers same canonical identity");
    for (auto result : {kResultFalse, kNotImplemented, kNoInterface}) {
        component.result = result; const int old = controller.calls; send();
        require(controller.calls == old + 1, "controller fallback");
    }
    component.result = kInvalidArgument; const int old = controller.calls; send();
    require(controller.calls == old, "payload rejection not hidden");
    component.supported = false; send(); require(controller.calls == old + 1, "component no interface fallback");
    controller.supported = false; const int both = component.calls + controller.calls; send();
    require(component.calls + controller.calls == both, "no interfaces");
    component.supported = true; component.result = kResultOk; send();
    auto retained = component.held;
    s.title = L"changed"; s.artwork.assign(3, 9);
    wchar_t text[8194]{};
    require(retained->getAttributes()->getString("title", reinterpret_cast<TChar*>(text), sizeof(text)) == kResultOk && std::wstring(text).find(L"日本語") == 0, "SDK string copy lifetime");
    const void* binary = nullptr; uint32 bytes = 0;
    require(retained->getAttributes()->getBinary("artwork", binary, bytes) == kResultOk && bytes == 3 && static_cast<const uint8_t*>(binary)[0] == 1, "SDK binary copy lifetime");
    for (const auto& key : {"allocate", "title", "album", "artist", "artwork", "artworkMimeType"}) {
        auto failing = owned(new FailingHost); failing->key = key;
        const int before = component.calls;
        notify_now_playing(failing, &component, &controller, s);
        require(component.calls == before, "setter/allocation failure stops notify");
    }
    NowPlayingSnapshot empty;
    notify_now_playing(host, &component, &controller, empty);
    require(component.held->getAttributes()->getBinary("artwork", binary, bytes) != kResultOk, "clear omits artwork");
    require(component.held->getAttributes()->getString("title", reinterpret_cast<TChar*>(text), sizeof(text)) == kResultOk && text[0] == 0, "clear text empty");
}
static void mailbox_tests() {
    NowPlayingRegistry registry; registry.add(0, 0);
    auto owner = registry.find(0); int wakes = 0;
    auto wake = [&](auto) { ++wakes; };
    require(admit_now_playing_frame(frame(10), registry, wake) && wakes == 1, "initial wake");
    auto running = owner->begin(100);
    require(running && running->revision == 10, "begin running");
    std::weak_ptr<const NowPlayingSnapshot> lifetime = running;
    require(admit_now_playing_frame(frame(12), registry, wake), "clear admitted");
    require(admit_now_playing_frame(frame(11, "old"), registry, wake), "stale parsed");
    require(wakes == 1 && owner->stats().executing, "running pending no wake");
    running.reset(); require(!lifetime.expired(), "running owned through close");
    const auto epoch = registry.seal_all();
    require(owner->stats().executing && !owner->expired(5099) && owner->expired(5100), "closed running watchdog");
    registry.add(0, epoch); auto next = registry.find(0);
    require(next && next != owner, "new incarnation");
    require(admit_now_playing_frame(frame(1, "new"), registry, wake), "new revision reset");
    require(!owner->finish() && lifetime.expired(), "old completion frees without wake");
    auto latest = next->begin(200); require(latest && latest->revision == 1, "new survives old completion");
    next->finish(); latest.reset();
    require(!admit_now_playing_frame(frame(2, "", "", "", 999), registry, wake), "unknown slot");
    registry.seal_all(); registry.add(0, epoch); require(!registry.find(0), "queued old initialization cannot reopen sealed admission");
    // Scheduled clear supersedes old image; newer media may supersede clear.
    const auto current = registry.epoch(); registry.add(0, current); auto active = registry.find(0);
    admit_now_playing_frame(frame(20, "old", "AA==", "image/png"), registry, wake);
    admit_now_playing_frame(frame(21), registry, wake);
    admit_now_playing_frame(frame(20, "stale", "AA==", "image/png"), registry, wake);
    auto cleared = active->begin(1000); require(cleared && cleared->revision == 21 && cleared->artwork.empty(), "clear watermark");
    admit_now_playing_frame(frame(22, "new"), registry, wake); active->finish(); cleared.reset();
    auto media = active->begin(1100); require(media && media->revision == 22, "clear then new media");
    active->finish(); media.reset(); registry.seal_all();
    require(registry.watched().empty(), "all payload owners released");
}


static std::string extended_frame(const std::string& extra) {
    auto json = frame(1); json.pop_back(); return json + "," + extra + "}";
}
static std::string position_frame(uint64_t generation, double position = 0, bool available = true) {
    return "{\"cmd\":\"set_playback_position\",\"slot_id\":0,\"mediaGeneration\":" + std::to_string(generation) +
        ",\"available\":" + (available ? "1" : "0") + ",\"playing\":0,\"positionSeconds\":" +
        std::to_string(position) + ",\"rate\":2.0,\"hostTimeNanoseconds\":18446744073709551615}";
}
static void lyrics_parser_tests() {
    NowPlayingSnapshot snapshot;
    require(parse_now_playing(extended_frame("\"mediaGeneration\":18446744073709551615,\"durationSeconds\":6e1,\"lyricsSidecar\":\"[00:01]line\",\"lyricsEmbedded\":\"tag\""), snapshot), "all optional attributes");
    require(snapshot.media_generation == UINT64_MAX && snapshot.duration_seconds == 60 && snapshot.lyrics_sidecar == L"[00:01]line" && snapshot.lyrics_embedded == L"tag", "optional values");
    require(parse_now_playing(frame(2), snapshot) && !snapshot.media_generation && !snapshot.duration_seconds && !snapshot.lyrics_sidecar && !snapshot.lyrics_embedded, "optional absence resets snapshot");
    require(parse_now_playing(extended_frame("\"mediaGeneration\":null,\"durationSeconds\":null,\"lyricsSidecar\":null,\"lyricsEmbedded\":null"), snapshot) && !snapshot.lyrics_sidecar, "null optional values");
    for (size_t units : {NOW_PLAYING_LYRICS_MAX - 1, NOW_PLAYING_LYRICS_MAX, NOW_PLAYING_LYRICS_MAX + 1}) {
        require(parse_now_playing(extended_frame("\"lyricsSidecar\":\"" + std::string(units, 'a') + "\""), snapshot), "lyrics boundary accepted");
        require(snapshot.lyrics_sidecar->size() == std::min(units, NOW_PLAYING_LYRICS_MAX), "lyrics UTF16 limit");
    }
    require(parse_now_playing(extended_frame("\"lyricsSidecar\":\"" + std::string(NOW_PLAYING_LYRICS_MAX - 1, 'a') + "\\ud83d\\ude00\""), snapshot) && snapshot.lyrics_sidecar->size() == NOW_PLAYING_LYRICS_MAX - 1, "lyrics surrogate boundary");
    for (const auto* invalid : {"\"lyricsSidecar\":\"\\ud800\"", "\"lyricsEmbedded\":\"\\u0000\"", "\"mediaGeneration\":-1", "\"durationSeconds\":-1", "\"durationSeconds\":1e999", "\"durationSeconds\":01", "\"durationSeconds\":1.", "\"lyricsSidecar\":false", "\"mediaGeneration\":1,\"mediaGeneration\":2"})
        require(!parse_now_playing(extended_frame(invalid), snapshot), "invalid optional rejected");
    auto maximum = frame(1, std::string(4096, 'a'), maximum_art(), "image/png"); maximum.pop_back();
    std::string escaped;
    for (size_t n = 0; n < NOW_PLAYING_LYRICS_MAX; ++n) escaped += "\\u0001";
    maximum += ",\"mediaGeneration\":18446744073709551615,\"lyricsSidecar\":\"" + escaped + "\",\"lyricsEmbedded\":\"" + escaped + "\",\"durationSeconds\":60}";
    require(maximum.size() < MAX_CONTROL_MSG_SIZE && parse_now_playing(maximum, snapshot), "max image plus both maximally escaped lyrics within frame");
    require(snapshot.artwork.size() == NOW_PLAYING_IMAGE_MAX && snapshot.lyrics_sidecar->size() == NOW_PLAYING_LYRICS_MAX && snapshot.lyrics_embedded->size() == NOW_PLAYING_LYRICS_MAX, "maximum combined decoded sizes");
    maximum.pop_back(); maximum += ",";
    require(!parse_now_playing(maximum, snapshot), "twelve fields require closing brace");
}
static void playback_tests() {
    PlaybackPositionSnapshot parsed;
    require(parse_playback_position(position_frame(UINT64_MAX, 2.5), parsed) && parsed.media_generation == UINT64_MAX && parsed.host_time_ns == UINT64_MAX && parsed.position_seconds == 2.5 && parsed.available && !parsed.playing && parsed.rate == 2, "playback parser");
    for (const auto& replace : {std::pair<std::string, std::string>{"available\":1", "available\":2"}, {"2.500000", "-1"}, {"2.0", "0"}, {"2.0", "1e999"}, {"18446744073709551615", "18446744073709551616"}, {"\"playing\"", "\"available\""}}) {
        auto invalid = position_frame(1, 2.5); auto offset = invalid.find(replace.first); require(offset != std::string::npos, "mutation field exists"); invalid.replace(offset, replace.first.size(), replace.second);
        require(!parse_playback_position(invalid, parsed), "invalid playback rejected");
    }
    auto host = owned(new Steinberg::Vst::HostApplication);
    Receiver component, controller;
    component.expected_id = controller.expected_id = "EffeTune.PlaybackPosition";
    require(parse_playback_position(position_frame(UINT64_MAX, 2.5), parsed), "playback reparsed");
    notify_playback_position(host, &component, &controller, parsed);
    require(component.calls == 1 && controller.calls == 0 && component.thread == GetCurrentThreadId(), "playback component on caller GUI thread");
    auto* attrs = component.held->getAttributes(); int64 integer = 0; double real = 0;
    require(attrs->getInt("mediaGeneration", integer) == kResultOk && integer == -1, "generation bit pattern");
    require(attrs->getInt("hostTimeNanoseconds", integer) == kResultOk && integer == -1, "QPC bit pattern");
    require(attrs->getInt("available", integer) == kResultOk && integer == 1 && attrs->getInt("playing", integer) == kResultOk && integer == 0, "playback booleans use integers");
    require(attrs->getFloat("positionSeconds", real) == kResultOk && real == 2.5 && attrs->getFloat("rate", real) == kResultOk && real == 2, "playback floats");
    component.result = kNotImplemented;
    notify_playback_position(host, &component, &controller, parsed);
    require(controller.calls == 1, "playback controller fallback");
    notify_playback_position(host, &component, &component, parsed);
    require(component.calls == 3, "playback canonical identity notified once");
    component.result = kResultOk; parsed.available = false;
    notify_playback_position(host, &component, &controller, parsed);
    require(component.held->getAttributes()->getInt("available", integer) == kResultOk && integer == 0, "unavailable clears playback");
    for (const auto* key : {"mediaGeneration", "available", "playing", "positionSeconds", "rate", "hostTimeNanoseconds"}) {
        auto failing = owned(new FailingHost); failing->key = key; const int before = component.calls;
        notify_playback_position(failing, &component, &controller, parsed);
        require(component.calls == before, "playback setter failure stops notify");
    }
    NowPlayingSnapshot text; text.media_generation = UINT64_MAX; text.lyrics_sidecar = L"sidecar"; text.lyrics_embedded = L"embedded"; text.duration_seconds = 60;
    component.expected_id = controller.expected_id = "EffeTune.NowPlaying"; component.result = kResultOk;
    notify_now_playing(host, &component, &controller, text);
    attrs = component.held->getAttributes(); wchar_t lyrics[32]{};
    require(attrs->getInt("mediaGeneration", integer) == kResultOk && integer == -1 && attrs->getFloat("durationSeconds", real) == kResultOk && real == 60, "NowPlaying typed trial attributes");
    require(attrs->getString("lyricsSidecar", reinterpret_cast<TChar*>(lyrics), sizeof(lyrics)) == kResultOk && std::wstring(lyrics) == L"sidecar", "sidecar string");
    require(attrs->getString("lyricsEmbedded", reinterpret_cast<TChar*>(lyrics), sizeof(lyrics)) == kResultOk && std::wstring(lyrics) == L"embedded", "embedded string");
    for (const auto* key : {"mediaGeneration", "lyricsSidecar", "lyricsEmbedded", "durationSeconds"}) {
        auto failing = owned(new FailingHost); failing->key = key; const int before = component.calls;
        notify_now_playing(failing, &component, &controller, text);
        require(component.calls == before, "optional setter failure stops notify");
    }
    notify_now_playing(host, &component, &controller, NowPlayingSnapshot{});
    require(component.held->getAttributes()->getInt("mediaGeneration", integer) != kResultOk && component.held->getAttributes()->getString("lyricsSidecar", reinterpret_cast<TChar*>(lyrics), sizeof(lyrics)) != kResultOk, "missing trial attributes omitted from message");

    NowPlayingRegistry media; PlaybackPositionRegistry positions; media.add(0, 0); positions.add(0, 0);
    int wakes = 0; auto wake = [&](auto) { ++wakes; };
    auto a = std::make_shared<NowPlayingSnapshot>(); a->media_generation = 1; a->revision = 1;
    auto m = media.find(0); require(m->admit(a), "media A scheduled"); auto running_media = m->begin(100);
    require(admit_playback_position_frame(position_frame(2, 5), positions, wake), "position B during media A notify");
    auto b = std::make_shared<NowPlayingSnapshot>(); b->media_generation = 2; b->revision = 2; m->admit(b);
    auto pos = positions.find(0); auto running = pos->begin(200);
    require(running && running->media_generation != *running_media->media_generation, "position can precede media; plugin generation must gate");
    require(admit_playback_position_frame(position_frame(2, 6), positions, wake) && admit_playback_position_frame(position_frame(2, 7), positions, wake) && wakes == 1, "position running keeps latest pending without more tokens");
    require(pos->finish(), "position completion schedules pending"); auto latest = pos->begin(300);
    require(latest->position_seconds == 7, "position latest wins even same generation");
    require(m->finish(), "new media scheduled after A"); auto next_media = m->begin(300);
    require(next_media->media_generation == latest->media_generation, "media B and latest position B match");
    const auto epoch = positions.seal_all(); require(pos->stats().executing && pos->expired(5300), "closed playback still watched");
    positions.add(0, epoch); require(positions.find(0) != pos, "position fresh slot incarnation");
    require(!pos->finish(), "closed playback cannot repost"); positions.seal_all(); positions.add(0, epoch);
    require(!positions.find(0), "old queued initialization cannot reopen playback lane");
    m->finish(); media.seal_all(); require(positions.watched().empty() && media.watched().empty(), "both lanes release owners");

    // Exercise the shared executor on an inert dedicated GUI worker.
    auto thread_owner = std::make_shared<PlaybackPositionMailbox>(0);
    auto thread_snapshot = std::make_shared<PlaybackPositionSnapshot>(); thread_snapshot->media_generation = 7;
    thread_owner->admit(thread_snapshot); component.expected_id = "EffeTune.PlaybackPosition";
    std::thread gui([&] {
        execute_now_playing(thread_owner, GetTickCount64(), [&](const auto& value) {
            notify_playback_position(host, &component, nullptr, value);
        }, [](auto) {});
    });
    gui.join(); require(component.thread != GetCurrentThreadId() && !thread_owner->stats().executing, "position notify uses GUI executor thread");
}

// Holds a real packaged-task future. Begin/finish is injected exactly after the
// helper's initial Scheduled snapshot and before its first positive wait, while
// the operation itself stays gated until that wait has timed out. This makes
// the Scheduled -> Running -> Idle boundary deterministic without timing sleeps.
class NotifyDuringFutureWait {
public:
    std::future<int>& future;
    std::shared_ptr<NowPlayingMailbox> mailbox;
    bool began = false, timed_out_idle = false;
    std::future_status wait_for(std::chrono::milliseconds duration) {
        if (duration.count() > 0 && !began) {
            began = true;
            require(mailbox->stats().phase == NowPlayingMailbox::Phase::Scheduled, "initial Scheduled boundary");
            execute_now_playing(mailbox, GetTickCount64(), [](const auto&) {}, [](auto) {});
        }
        const auto result = future.wait_for(duration);
        if (duration.count() > 0 && result == std::future_status::timeout) {
            const auto stats = mailbox->stats();
            timed_out_idle = stats.phase == NowPlayingMailbox::Phase::Idle && stats.execution_count == 1;
        }
        return result;
    }
};
static void completed_notify_budget_test(bool replace_owner) {
    auto owner = std::make_shared<NowPlayingMailbox>(0);
    auto snapshot = std::make_shared<NowPlayingSnapshot>(); snapshot->revision = 1; owner->admit(snapshot);
    auto other_incarnation = std::make_shared<NowPlayingMailbox>(0);
    other_incarnation->admit(snapshot);
    execute_now_playing(other_incarnation, GetTickCount64(), [](const auto&) {}, [](auto) {});
    std::promise<void> allow_operation;
    auto gate = allow_operation.get_future();
    std::packaged_task<int()> task([&] { gate.wait(); return 42; });
    auto future = task.get_future();
    std::thread gui([&] { task(); });
    NotifyDuringFutureWait waiting{future, owner};
    int lookups = 0;
    auto result = await_gui_future(waiting, std::chrono::milliseconds(10), [&] {
        if (++lookups == 2) {
            require(waiting.timed_out_idle && future.wait_for(std::chrono::milliseconds(0)) == std::future_status::timeout,
                "notification ended while same operation still waiting");
            allow_operation.set_value();
            return replace_owner ? other_incarnation : owner;
        }
        return owner;
    });
    gui.join();
    require(lookups == 2 && waiting.timed_out_idle && future.get() == 42, "real future deterministic boundary");
    require(result.outcome == (replace_owner ? GuiFutureWait::GuiTimeout : GuiFutureWait::Ready),
        "completed notify restores budget only for same canonical incarnation");
}
static void gui_future_tests() {
    auto owner = std::make_shared<NowPlayingMailbox>(0);
    auto snapshot = std::make_shared<NowPlayingSnapshot>(); snapshot->revision = 1;
    owner->admit(snapshot); owner->begin(GetTickCount64()); owner->close();
    std::promise<int> already_done; auto ready = already_done.get_future(); already_done.set_value(42);
    auto result = await_gui_future(ready, std::chrono::milliseconds(10), [&] { return owner; });
    require(result.outcome == GuiFutureWait::Ready && ready.get() == 42 && owner->stats().executing, "ready future bypasses metadata wait");
    std::promise<void> operation; auto future = operation.get_future();
    std::thread receiver([&] {
        std::mutex mutex; std::condition_variable cv; std::unique_lock<std::mutex> lock(mutex);
        cv.wait_for(lock, std::chrono::milliseconds(40));
        owner->finish(); operation.set_value();
    });
    result = await_gui_future(future, std::chrono::milliseconds(10), [&] { return owner; });
    receiver.join();
    require(result.outcome == GuiFutureWait::Ready, "Closed Running completes past ordinary sync budget");
    future.get();
    std::promise<void> hung_operation; auto ordinary = hung_operation.get_future();
    result = await_gui_future(ordinary, std::chrono::milliseconds(10), [] { return std::shared_ptr<NowPlayingMailbox>{}; });
    require(result.outcome == GuiFutureWait::GuiTimeout, "ordinary GUI timeout policy preserved");
    auto scheduled = std::make_shared<NowPlayingMailbox>(1); scheduled->admit(snapshot);
    result = await_gui_future(ordinary, std::chrono::milliseconds(10), [&] { return scheduled; });
    require(result.outcome == GuiFutureWait::GuiTimeout, "Scheduled control wake is never waited");
    completed_notify_budget_test(false);
    completed_notify_budget_test(true);

    auto media = std::make_shared<NowPlayingMailbox>(0);
    auto position = std::make_shared<PlaybackPositionMailbox>(0);
    auto pos = std::make_shared<PlaybackPositionSnapshot>(); position->admit(pos);
    std::promise<int> operation_done; auto operation_future = operation_done.get_future();
    struct PositionDuringWait {
        std::future<int>& future;
        std::shared_ptr<PlaybackPositionMailbox> owner;
        bool began = false;
        std::future_status wait_for(std::chrono::milliseconds duration) {
            if (duration.count() > 0 && !began) {
                began = true;
                execute_now_playing(owner, GetTickCount64(), [](const auto&) {}, [](auto) {});
            }
            return future.wait_for(duration);
        }
    } waiting{operation_future, position};
    int lookups = 0;
    result = await_gui_notifications(waiting, std::chrono::milliseconds(1), [&] {
        if (++lookups == 2) operation_done.set_value(1);
        return std::vector<std::shared_ptr<NotificationMailbox>>{media, position};
    });
    require(result.outcome == GuiFutureWait::Ready && lookups == 2 && position->stats().execution_count == 1, "position lane completed during wait restores same operation budget");
    position->admit(pos); position->begin(GetTickCount64() - NOW_PLAYING_TIMEOUT_MS); position->close();
    result = await_gui_notifications(ordinary, std::chrono::milliseconds(1), [&] {
        return std::vector<std::shared_ptr<NotificationMailbox>>{media, position};
    });
    require(result.outcome == GuiFutureWait::NowPlayingTimeout && result.mailbox == position, "closed hung position owns GUI timeout");
    position->finish();
}

// A headless child host for actual stdin-frame -> reader -> shared mailbox ->
// control wake -> GUI receiver tests. Shares all production admission/execution
// and SDK notify code; the only replacement is the inert GUI executor/receiver.
static int headless_host(bool hang, bool watchdog) {
    _setmode(_fileno(stdin), _O_BINARY);
    NowPlayingRegistry registry; registry.add(0, 0);
    std::mutex mutex;
    std::condition_variable cv;
    struct Control { std::shared_ptr<NowPlayingMailbox> mailbox; std::string operation; };
    std::deque<Control> control;
    std::deque<std::shared_ptr<NowPlayingMailbox>> gui;
    std::weak_ptr<NowPlayingMailbox> gui_mailbox;
    std::deque<std::function<void()>> gui_state_tasks;
    bool stopped = false, released = false;
    std::atomic<bool> monitoring{true};
    std::atomic<size_t> high_running{0}, high_pending{0}, high_wakes{0}, high_bytes{0}, high_frame{0};
    std::atomic<bool> get_state_queued{false}, get_state_done{false};
    auto host = owned(new Steinberg::Vst::HostApplication);
    Receiver receiver;
    receiver.entered = [&] {
        std::fprintf(stderr, "FIXTURE notify_entered\n"); std::fflush(stderr);
        std::unique_lock<std::mutex> lock(mutex);
        cv.wait(lock, [&] { return released && !hang; });
    };
    auto wake = [&](auto mailbox) { std::lock_guard<std::mutex> lock(mutex); control.push_back({mailbox, {}}); cv.notify_all(); };
    std::thread monitor([&] {
        while (monitoring) {
            for (const auto& owner : registry.watched()) if (watchdog && owner->expired(GetTickCount64())) {
                std::fprintf(stderr, "FIXTURE watchdog expired get_state_queued=%d done=%d\n", int(get_state_queued), int(get_state_done));
                std::fflush(stderr); terminate_now_playing_timeout(owner);
            }
            std::unique_lock<std::mutex> lock(mutex);
            cv.wait_for(lock, std::chrono::milliseconds(20));
        }
    });
    std::thread dispatcher([&] {
        std::unique_lock<std::mutex> lock(mutex);
        for (;;) {
            cv.wait(lock, [&] { return stopped || !control.empty(); });
            if (stopped && control.empty()) break;
            auto command = std::move(control.front()); control.pop_front();
            if (command.mailbox) {
                gui_mailbox = command.mailbox;
                gui.push_back(std::move(command.mailbox)); cv.notify_all();
                continue;
            }
            const auto operation = command.operation;
            const auto timeout = std::chrono::milliseconds(operation == "query" ? 200 : 5000);
            // Real packaged_task/future path used by both production sync helpers.
            auto task = std::make_shared<std::packaged_task<void()>>([&, operation] {
                if (operation == "query") get_state_done = true;
            });
            auto future = task->get_future();
            gui_state_tasks.push_back([task] { (*task)(); });
            auto bound_mailbox = gui_mailbox;
            cv.notify_all(); lock.unlock();
            const auto result = await_gui_future(future, timeout, [bound_mailbox] { return bound_mailbox.lock(); });
            if (result.outcome == GuiFutureWait::NowPlayingTimeout) {
                std::fprintf(stderr, "FIXTURE watchdog expired get_state_queued=%d done=%d operation=%s\n",
                    int(get_state_queued), int(get_state_done), operation.c_str()); std::fflush(stderr);
                terminate_now_playing_timeout(result.mailbox);
            }
            if (result.outcome == GuiFutureWait::GuiTimeout) {
                std::printf("GENERIC_ERROR\n"); std::fflush(stdout); ExitProcess(1);
            }
            future.get();
            std::fprintf(stderr, "FIXTURE sync_complete operation=%s\n", operation.c_str()); std::fflush(stderr);
            lock.lock();
        }
    });
    std::thread gui_thread([&] {
        for (;;) {
            std::shared_ptr<NowPlayingMailbox> owner;
            std::function<void()> state_task;
            {
                std::unique_lock<std::mutex> lock(mutex);
                cv.wait(lock, [&] { return stopped || !gui.empty() || !gui_state_tasks.empty(); });
                if (!gui.empty()) { owner = std::move(gui.front()); gui.pop_front(); }
                else if (!gui_state_tasks.empty()) { state_task = std::move(gui_state_tasks.front()); gui_state_tasks.pop_front(); }
                else if (stopped) break;
            }
            if (state_task) { state_task(); continue; }
            execute_now_playing(owner, GetTickCount64(), [&](const auto& snapshot) {
                notify_now_playing(host, &receiver, nullptr, snapshot);
                std::fprintf(stderr, "FIXTURE applied revision=%llu\n", static_cast<unsigned long long>(snapshot.revision)); std::fflush(stderr);
            }, wake);
        }
    });
    std::string input;
    while (read_control_frame([](void* data, size_t n) { return std::fread(data, 1, n, stdin) == n; }, input)) {
        const auto command = extract_json_string_field(input, "cmd");
        high_frame = std::max(high_frame.load(), input.size());
        if (command == "release") { std::lock_guard<std::mutex> lock(mutex); released = true; cv.notify_all(); }
        else if (command == "query_state") {
            get_state_queued = true;
            std::lock_guard<std::mutex> lock(mutex);
            control.push_back({{}, "query"}); cv.notify_all();
        } else if (command == "query_state_concurrent") {
            get_state_queued = true;
            std::lock_guard<std::mutex> lock(mutex);
            gui_state_tasks.push_back([&] { get_state_done = true; }); cv.notify_all();
        } else if (command == "close") {
            registry.seal_all();
            std::lock_guard<std::mutex> lock(mutex);
            control.push_back({{}, "unload"}); cv.notify_all();
        }
        else if (command == "open") { const auto epoch = registry.seal_all(); registry.add(0, epoch); }
        else {
            require(admit_now_playing_frame(input, registry, wake), "fixture admission");
            size_t running = 0, pending = 0, bytes = 0;
            for (const auto& mailbox : registry.watched()) {
                auto stats = mailbox->stats();
                running += stats.executing ? 1 : 0;
                pending += stats.pending_bytes ? 1 : 0;
                bytes += stats.running_bytes + stats.pending_bytes;
            }
            high_running = std::max(high_running.load(), running);
            high_pending = std::max(high_pending.load(), pending);
            high_bytes = std::max(high_bytes.load(), bytes);
            std::lock_guard<std::mutex> lock(mutex);
            const size_t wakes = gui.size() + static_cast<size_t>(std::count_if(control.begin(), control.end(), [](const auto& c) { return bool(c.mailbox); }));
            high_wakes = std::max(high_wakes.load(), wakes);
            require(running <= 1 && pending <= 1 && bytes <= 2 * NOW_PLAYING_IMAGE_MAX && wakes <= 1, "mailbox high water");
            // Includes reader candidate, frame and SDK message copy in addition
            // to measured mailbox retention, including replacement overlap.
            require(bytes + NOW_PLAYING_IMAGE_MAX * 2 + input.size() <= 64 * 1024 * 1024, "total image/frame budget");
        }
        input.clear();
    }
    registry.seal_all();
    { std::lock_guard<std::mutex> lock(mutex); stopped = true; cv.notify_all(); }
    dispatcher.join(); gui_thread.join(); monitoring = false; cv.notify_all(); monitor.join(); receiver.held = nullptr;
    require(receiver.thread != GetCurrentThreadId(), "notify on dedicated fake GUI thread");
    require(registry.watched().empty(), "fixture slot payload release");
    std::fprintf(stderr, "FIXTURE high running=%zu pending=%zu wakes=%zu mailbox_bytes=%zu frame=%zu generic_image_bytes=0 released=1\n",
        high_running.load(), high_pending.load(), high_wakes.load(), high_bytes.load(), high_frame.load());
    return 0;
}
int main(int argc, char** argv) {
    try {
        if (argc > 1) return headless_host(std::strcmp(argv[1], "--slow") != 0,
            std::strcmp(argv[1], "--hang-sync") != 0);
        parser_tests(); lyrics_parser_tests(); receiver_tests(); mailbox_tests(); playback_tests(); gui_future_tests();
        std::cout << "Now Playing parser/SDK receiver/mailbox tests PASS\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << "FAIL: " << error.what() << '\n'; return 1;
    }
}
