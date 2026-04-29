// VST3 IHostApplication / IComponentHandler 最小実装

#include "host_app.h"

#include <cstring>
#include <cwchar>

#include "pluginterfaces/base/funknownimpl.h"
#include "public.sdk/source/vst/hosting/hostclasses.h"

namespace miv {

using namespace Steinberg;

// IHostApplication 実装
HostApplication::HostApplication() {
    FUNKNOWN_CTOR
}

IMPLEMENT_FUNKNOWN_METHODS(HostApplication, Vst::IHostApplication, Vst::IHostApplication::iid)

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

// IPlugFrame 最小実装
IMPLEMENT_FUNKNOWN_METHODS(PlugFrame, Steinberg::IPlugFrame, Steinberg::IPlugFrame::iid)

tresult PLUGIN_API PlugFrame::resizeView(Steinberg::IPlugView* view,
                                          Steinberg::ViewRect* newSize) {
    // プラグインからリサイズ要求が来た。
    // Phase 0b ではホストウィンドウサイズを変えないが、kResultOk を返さないと
    // プラグインが「リサイズ拒否された」と判断して描画を保留することがある。
    // VST3 仕様上は host がサイズ変更を実行してから view->onSize(newSize) を
    // 呼び返すのが正しい。tester では view->onSize(newSize) だけ呼ぶ。
    if (view && newSize) {
        view->onSize(newSize);
    }
    return kResultOk;
}

// IComponentHandler 最小実装
// POC では parameter 自動化のフィードバックは無視する (= no-op)。
IMPLEMENT_FUNKNOWN_METHODS(ComponentHandler, Vst::IComponentHandler, Vst::IComponentHandler::iid)

tresult PLUGIN_API ComponentHandler::beginEdit(Vst::ParamID /*id*/) {
    return kResultOk;
}

tresult PLUGIN_API ComponentHandler::performEdit(Vst::ParamID /*id*/, Vst::ParamValue /*value*/) {
    return kResultOk;
}

tresult PLUGIN_API ComponentHandler::endEdit(Vst::ParamID /*id*/) {
    return kResultOk;
}

tresult PLUGIN_API ComponentHandler::restartComponent(int32 /*flags*/) {
    // restartComponent(kLatencyChanged) を受けたら親に latency_changed を通知すべきだが、
    // POC では無視 (Phase B で実装)。
    return kResultOk;
}

}  // namespace miv
