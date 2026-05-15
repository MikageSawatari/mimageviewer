// VST3 IHostApplication / IComponentHandler 最小実装

#include "host_app.h"

#include <algorithm>
#include <cstring>
#include <cwchar>

#include <windows.h>

#include "pluginterfaces/base/funknownimpl.h"
#include "public.sdk/source/vst/hosting/hostclasses.h"
#include "public.sdk/source/vst/hosting/pluginterfacesupport.h"

namespace miv {

using namespace Steinberg;

namespace {
constexpr UINT kBridgeResizePluginClientMsg = WM_APP + 0x4D9;
}

// IHostApplication 実装
HostApplication::HostApplication() {
    plug_iface_support_ = Steinberg::owned(new Vst::PlugInterfaceSupport);
}

// FUnknown の addRef/release は atomic ref count (T10 v0.9.0)。
// VST3 プラグインは任意スレッドから addRef/release を呼びうるため、
// std::atomic + acq_rel ordering で正しく動作させる。release の戻り値が 0 に
// なった瞬間に delete this する古典的 COM-style refcount。
Steinberg::uint32 PLUGIN_API HostApplication::addRef() {
    auto prev = ref_count_.fetch_add(1, std::memory_order_acq_rel);
    return static_cast<Steinberg::uint32>(prev + 1);
}
Steinberg::uint32 PLUGIN_API HostApplication::release() {
    auto prev = ref_count_.fetch_sub(1, std::memory_order_acq_rel);
    auto cnt = prev - 1;
    if (cnt == 0) {
        delete this;
        return 0;
    }
    return static_cast<Steinberg::uint32>(cnt);
}

// queryInterface: IHostApplication / IPlugInterfaceSupport / FUnknown を返す。
// IPlugInterfaceSupport を返すことで、プラグインから「ホストはどの拡張 IF を
// サポートするか」を問い合わせられた時に応答できる。これがないと一部の
// プラグイン (Pro-Q 4 等) が機能制限モードで動作する。
tresult PLUGIN_API HostApplication::queryInterface(const TUID _iid, void** obj) {
    if (!obj) return kInvalidArgument;
    if (FUnknownPrivate::iidEqual(_iid, FUnknown::iid) ||
        FUnknownPrivate::iidEqual(_iid, Vst::IHostApplication::iid)) {
        addRef();
        *obj = static_cast<Vst::IHostApplication*>(this);
        return kResultOk;
    }
    if (FUnknownPrivate::iidEqual(_iid, Vst::IPlugInterfaceSupport::iid)) {
        if (plug_iface_support_) {
            plug_iface_support_->addRef();
            *obj = static_cast<Vst::IPlugInterfaceSupport*>(plug_iface_support_.get());
            return kResultOk;
        }
    }
    *obj = nullptr;
    return kNoInterface;
}

tresult PLUGIN_API HostApplication::getName(Vst::String128 name) {
    // "mImageViewer-VST3-Host" の UTF-16
    static const char16_t kName[] = u"mImageViewer-VST3-Host";
    size_t len = 0;
    while (kName[len] != 0 && len < 127) ++len;
    std::memcpy(name, kName, (len + 1) * sizeof(char16_t));
    return kResultOk;
}

tresult PLUGIN_API HostApplication::createInstance(TUID cid, TUID iid, void** obj) {
    // VST3 仕様: host が IMessage / IAttributeList を生成する責務を持つ。
    // これは VST3 SDK の Hosting::HostMessage / HostAttributeList を流用する。
    if (FUnknownPrivate::iidEqual(cid, Vst::IMessage::iid) &&
        FUnknownPrivate::iidEqual(iid, Vst::IMessage::iid)) {
        *obj = static_cast<Vst::IMessage*>(new Vst::HostMessage);
        return kResultOk;
    }
    if (FUnknownPrivate::iidEqual(cid, Vst::IAttributeList::iid) &&
        FUnknownPrivate::iidEqual(iid, Vst::IAttributeList::iid)) {
        if (auto al = Vst::HostAttributeList::make()) {
            al->addRef();
            *obj = al.get();
            return kResultOk;
        }
        return kOutOfMemory;
    }
    *obj = nullptr;
    return kResultFalse;
}

// IPlugFrame 実装
IMPLEMENT_FUNKNOWN_METHODS(PlugFrame, Steinberg::IPlugFrame, Steinberg::IPlugFrame::iid)

void PlugFrame::mark_user_resize() {
    last_user_resize_tick_ = static_cast<uint64_t>(GetTickCount64());
}

void PlugFrame::set_user_resizing(bool active) {
    user_resizing_ = active;
    if (!active) {
        last_user_resize_tick_ = static_cast<uint64_t>(GetTickCount64());
    }
}

tresult PLUGIN_API PlugFrame::resizeView(Steinberg::IPlugView* view,
                                          Steinberg::ViewRect* newSize) {
    // プラグインからリサイズ要求が来た。VST3 仕様: host が
    // 1) ホストウィンドウを newSize に合わせてリサイズ (= AdjustWindowRectExForDpi
    //    でフレーム厚を加算した outer サイズで SetWindowPos)
    // 2) view->onSize(newSize) で受領を通知
    // 3) kResultOk を返す
    // という順序が必要。tester ではホスト HWND は別プロセスにあるが、
    // Win32 API はクロスプロセスの HWND 操作を許容するので直接 SetWindowPos する。
    if (!view || !newSize) {
        return kResultOk;
    }
    int32 w = newSize->right - newSize->left;
    int32 h = newSize->bottom - newSize->top;
    // ── フィードバックループ抑止 ──
    // WM_ENTERSIZEMOVE-EXITSIZEMOVE 中だけ SetWindowPos を抑止する。
    // 直近 WM_SIZE の時間ベース抑止は、プラグイン内蔵 resize handle から来る
    // 正規 resizeView まで捨ててしまい、外枠と editor 内部サイズの不一致を作る。
    bool suppressed = user_resizing_ || !host_resize_enabled_;
    if (suppressed || w <= 0 || h <= 0) {
        std::fprintf(stderr,
                     "[BRIDGE] resizeView: rect=(%d,%d,%d,%d) size=%dx%d suppressed=%d user_resizing=%d host_resize_enabled=%d last_user_resize_ms=%llu\n",
                     newSize->left,
                     newSize->top,
                     newSize->right,
                     newSize->bottom,
                     w,
                     h,
                     suppressed ? 1 : 0,
                     user_resizing_ ? 1 : 0,
                     host_resize_enabled_ ? 1 : 0,
                     static_cast<unsigned long long>(last_user_resize_tick_));
    }
    if (host_hwnd_ && w > 0 && h > 0 && !suppressed) {
        HWND hwnd = reinterpret_cast<HWND>(host_hwnd_);
        std::fprintf(stderr,
                     "[BRIDGE] resizeView apply: rect=(%d,%d,%d,%d) size=%dx%d\n",
                     newSize->left,
                     newSize->top,
                     newSize->right,
                     newSize->bottom,
                     w,
                     h);
        SendMessageW(hwnd,
                     kBridgeResizePluginClientMsg,
                     static_cast<WPARAM>(std::max<int32>(1, w)),
                     static_cast<LPARAM>(std::max<int32>(1, h)));
    }
    view->onSize(newSize);
    return kResultOk;
}

// IComponentHandler 最小実装
// POC では parameter 自動化のフィードバックは無視する (= no-op)。
IMPLEMENT_FUNKNOWN_METHODS(ComponentHandler, Vst::IComponentHandler, Vst::IComponentHandler::iid)

tresult PLUGIN_API ComponentHandler::beginEdit(Vst::ParamID /*id*/) {
    return kResultOk;
}

tresult PLUGIN_API ComponentHandler::performEdit(Vst::ParamID id, Vst::ParamValue value) {
    // UI スレッドからプラグイン経由で呼ばれる。同一 ParamID の連続更新は
    // last-write-wins で集約 (= map の operator[] で上書き)。これがないと
    // UI 高速ドラッグ時に同一 sampleOffset=0 に複数 point が積まれて
    // プラグインの補間器が振動 → クリックノイズになる。
    std::lock_guard<std::mutex> lk(pending_mutex_);
    pending_changes_[id] = value;
    return kResultOk;
}

void ComponentHandler::drain_into(Vst::IParameterChanges* output) {
    if (!output) return;
    std::unordered_map<Vst::ParamID, Vst::ParamValue> snapshot;
    {
        std::lock_guard<std::mutex> lk(pending_mutex_);
        if (pending_changes_.empty()) return;
        snapshot.swap(pending_changes_);
    }
    for (auto& [id, val] : snapshot) {
        Steinberg::int32 idx = 0;
        auto* queue = output->addParameterData(id, idx);
        if (queue) {
            Steinberg::int32 point = 0;
            queue->addPoint(0, val, point);
        }
    }
}

tresult PLUGIN_API ComponentHandler::endEdit(Vst::ParamID /*id*/) {
    return kResultOk;
}

tresult PLUGIN_API ComponentHandler::restartComponent(int32 flags) {
    // VST3 の restartComponent: プラグインから host への「再構成してください」通知。
    // flags は ivstcomponent.h の RestartFlags のビットマスク:
    //   - kReloadComponent (1)       : component の再ロードが必要
    //   - kIoChanged (1<<1)           : I/O bus が変わった
    //   - kParamValuesChanged (1<<2)  : パラメータ値が変わった (UI 同期目的)
    //   - kLatencyChanged (1<<3)      : ★ getLatencySamples() の戻り値が変わった
    //   - kParamTitlesChanged (1<<4)  : パラメータ表示名が変わった
    //   - kMidiCCAssignmentChanged (1<<5)
    //   - kNoteExpressionChanged (1<<6)
    //   - kIoTitlesChanged (1<<7)
    //   - kPrefOfKeySupportChanged (1<<8)
    //   - kRoutingInfoChanged (1<<9)
    //   - kKeyswitchChanged (1<<10)
    //
    // mIV では kLatencyChanged のみ重要 (PDC を再計算するため)。フラグを立てて
    // main thread の polling で getLatencySamples() を呼び直し、親に通知する。
    constexpr int32 kLatencyChanged = (1 << 3);
    if (flags & kLatencyChanged) {
        latency_changed_pending_.store(true, std::memory_order_release);
    }
    return kResultOk;
}

}  // namespace miv
