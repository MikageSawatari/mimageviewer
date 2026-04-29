// VST3 IHostApplication 実装と関連サービス。
//
// VST3 プラグインが host に問い合わせる API (`IHostApplication::createInstance`,
// `IHostApplication::getName`) と、parameter 変更時のフィードバック先 (`IComponentHandler`)
// を最低限実装する。parameter 自動化や preset 管理は v0.10.0 ではスコープ外。

#pragma once

#include "pluginterfaces/base/funknown.h"
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/vst/ivsthostapplication.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"

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

class ComponentHandler : public Steinberg::Vst::IComponentHandler {
public:
    ComponentHandler() = default;
    virtual ~ComponentHandler() = default;

    Steinberg::tresult PLUGIN_API beginEdit(Steinberg::Vst::ParamID id) override;
    Steinberg::tresult PLUGIN_API performEdit(Steinberg::Vst::ParamID id,
                                                Steinberg::Vst::ParamValue value) override;
    Steinberg::tresult PLUGIN_API endEdit(Steinberg::Vst::ParamID id) override;
    Steinberg::tresult PLUGIN_API restartComponent(Steinberg::int32 flags) override;

    DECLARE_FUNKNOWN_METHODS
};

}  // namespace miv
