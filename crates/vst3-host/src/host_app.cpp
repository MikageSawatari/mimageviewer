// VST3 IHostApplication / IComponentHandler 最小実装

#include "host_app.h"

#include <cstring>
#include <cwchar>

#include <windows.h>

#include "pluginterfaces/base/funknownimpl.h"
#include "public.sdk/source/vst/hosting/hostclasses.h"
#include "public.sdk/source/vst/hosting/pluginterfacesupport.h"

namespace miv {

using namespace Steinberg;

// IHostApplication 実装
HostApplication::HostApplication() {
    plug_iface_support_ = Steinberg::owned(new Vst::PlugInterfaceSupport);
}

// FUnknown の addRef/release は単純 atomic ref count
Steinberg::uint32 PLUGIN_API HostApplication::addRef() {
    return static_cast<Steinberg::uint32>(++ref_count_);
}
Steinberg::uint32 PLUGIN_API HostApplication::release() {
    auto cnt = --ref_count_;
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
    // session 終了時にもタイムスタンプを更新しておくと、直後の遅延 resizeView
    // が 250ms fallback で抑止される。Codex P4 「session 後の余韻」対応。
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
    // ── フィードバックループ抑止 (Insight2 リサイズ振動への対策) ──
    // ユーザーがホストウィンドウをドラッグしてリサイズ中、host は WM_SIZE 受信
    // ごとに `notify_host_resize` → mark_user_resize() で last_user_resize_tick_
    // を更新しながら view->onSize を呼ぶ。Insight2 はそれに対し、同期 / 非同期
    // (PostMessage) で resizeView を多発させてホスト窓のサイズ調整を要求して
    // くる。ここで SetWindowPos するとユーザーのドラッグと衝突して
    // ウィンドウが瞬間的にプラグイン推奨サイズへ吸着 → 次フレームでユーザー位置
    // に戻る → 暴れる。
    // 直近 250ms 以内に user resize があったときは SetWindowPos をスキップして
    // view->onSize で確認だけ返答する (= フィードバックループ抑止)。
    // Codex P4: session フラグ優先。WM_ENTERSIZEMOVE-EXITSIZEMOVE 中はずっと
    // SetWindowPos スキップ。fallback として 250ms タイムスタンプ抑止も併用
    // (= session 後の遅延 resizeView や session 経路を通らない場合の保険)。
    constexpr uint64_t SUPPRESS_WINDOW_MS = 250;
    bool suppressed = user_resizing_;
    if (!suppressed && last_user_resize_tick_ > 0) {
        uint64_t now = static_cast<uint64_t>(GetTickCount64());
        if (now - last_user_resize_tick_ < SUPPRESS_WINDOW_MS) {
            suppressed = true;
        }
    }
    if (host_hwnd_ && w > 0 && h > 0 && !suppressed) {
        HWND hwnd = reinterpret_cast<HWND>(host_hwnd_);
        UINT dpi = GetDpiForWindow(hwnd);
        if (dpi == 0) dpi = 96;
        RECT rect{0, 0, w, h};
        DWORD style = static_cast<DWORD>(GetWindowLongPtrW(hwnd, GWL_STYLE));
        DWORD ex_style = static_cast<DWORD>(GetWindowLongPtrW(hwnd, GWL_EXSTYLE));
        AdjustWindowRectExForDpi(&rect, style, FALSE, ex_style, dpi);
        int outer_w = rect.right - rect.left;
        int outer_h = rect.bottom - rect.top;
        SetWindowPos(hwnd, nullptr, 0, 0, outer_w, outer_h,
                      SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
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
