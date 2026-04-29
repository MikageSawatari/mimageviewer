// VST3 IHostApplication 実装と関連サービス。
//
// VST3 プラグインが host に問い合わせる API (`IHostApplication::createInstance`,
// `IHostApplication::getName`) と、parameter 変更時のフィードバック先 (`IComponentHandler`)
// を最低限実装する。parameter 自動化や preset 管理は v0.10.0 ではスコープ外。

#pragma once

#include "pluginterfaces/base/funknown.h"
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/gui/iplugview.h"
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

/// IPlugFrame の最小実装。
/// プラグインから「このサイズに変更して」と要求が来たときに `kResultOk` を
/// 返すだけのスタブ。実際にホストウィンドウのサイズを変えるには Rust 側に
/// IPC で通知する必要があるが、Phase 0b では描画開始に必要な最小機能のみ。
/// Pro-Q 4 など多くのプラグインは setFrame(frame) を呼ばないと描画を開始しない。
class PlugFrame : public Steinberg::IPlugFrame {
public:
    PlugFrame() = default;
    virtual ~PlugFrame() = default;

    Steinberg::tresult PLUGIN_API resizeView(Steinberg::IPlugView* view,
                                              Steinberg::ViewRect* newSize) override;

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
