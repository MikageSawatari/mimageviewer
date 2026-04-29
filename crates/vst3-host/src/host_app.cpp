// VST3 IHostApplication / IComponentHandler 最小実装

#include "host_app.h"

#include <cstring>
#include <cwchar>

#include <windows.h>

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

// IPlugFrame 実装
IMPLEMENT_FUNKNOWN_METHODS(PlugFrame, Steinberg::IPlugFrame, Steinberg::IPlugFrame::iid)

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
    if (host_hwnd_ && w > 0 && h > 0) {
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
