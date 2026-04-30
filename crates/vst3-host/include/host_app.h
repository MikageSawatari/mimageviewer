// VST3 IHostApplication 実装と関連サービス。
//
// VST3 プラグインが host に問い合わせる API (`IHostApplication::createInstance`,
// `IHostApplication::getName`) と、parameter 変更時のフィードバック先 (`IComponentHandler`)
// を最低限実装する。parameter 自動化や preset 管理は v0.10.0 ではスコープ外。

#pragma once

#include <mutex>
#include <utility>
#include <vector>

#include "pluginterfaces/base/funknown.h"
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/gui/iplugview.h"
#include "pluginterfaces/vst/ivsthostapplication.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstparameterchanges.h"

namespace miv {

class HostApplication : public Steinberg::Vst::IHostApplication {
public:
    HostApplication();
    virtual ~HostApplication() = default;

    // IHostApplication
    Steinberg::tresult PLUGIN_API getName(Steinberg::Vst::String128 name) override;
    Steinberg::tresult PLUGIN_API createInstance(Steinberg::TUID cid,
                                                  Steinberg::TUID iid,
                                                  void** obj) override;

    // FUnknown
    DECLARE_FUNKNOWN_METHODS
};

/// IPlugFrame 実装: プラグインから「このサイズに変更して」と要求が来たら、
/// 親 HWND (= tester プロセスのウィンドウ) を SetWindowPos でリサイズし、
/// view->onSize で受領を通知する。これが無いとプラグインのリサイズ追従や
/// 初期描画が動かない。
class PlugFrame : public Steinberg::IPlugFrame {
public:
    PlugFrame() = default;
    virtual ~PlugFrame() = default;

    /// show_gui で受け取った host HWND を保存しておく。
    /// resizeView で SetWindowPos するために必要 (= プロセス境界をまたぐ HWND
    /// 操作も Win32 API は許容)。
    void set_host_hwnd(void* hwnd) { host_hwnd_ = hwnd; }

    Steinberg::tresult PLUGIN_API resizeView(Steinberg::IPlugView* view,
                                              Steinberg::ViewRect* newSize) override;

    DECLARE_FUNKNOWN_METHODS

private:
    void* host_hwnd_ = nullptr;
};

class ComponentHandler : public Steinberg::Vst::IComponentHandler {
public:
    ComponentHandler() = default;
    virtual ~ComponentHandler() = default;

    Steinberg::tresult PLUGIN_API beginEdit(Steinberg::Vst::ParamID id) override;
    Steinberg::tresult PLUGIN_API performEdit(Steinberg::Vst::ParamID id,
                                                Steinberg::Vst::ParamValue value) override;
    Steinberg::tresult PLUGIN_API endEdit(Steinberg::Vst::ParamID id) override;
    Steinberg::tresult PLUGIN_API restartComponent(Steinberg::int32 flags) override;

    /// 蓄積された param 変更を ParameterChanges に投入する。
    /// audio thread が process 直前に呼んで、UI 由来のパラメータ変更を
    /// process に届ける。スレッド安全 (= mutex で保護)。
    void drain_into(Steinberg::Vst::IParameterChanges* output);

    DECLARE_FUNKNOWN_METHODS

private:
    std::mutex pending_mutex_;
    std::vector<std::pair<Steinberg::Vst::ParamID, Steinberg::Vst::ParamValue>> pending_changes_;
};

}  // namespace miv
