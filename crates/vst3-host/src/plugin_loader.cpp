// VST3 プラグインのロード & process loop 実装
//
// Phase 0 POC では「ロードしてパススルーで音を通す」のが目的。
// IComponent / IAudioProcessor / IEditController の取得と最低限の lifecycle 制御まで実装する。

#include "plugin_loader.h"

#include <algorithm>
#include <cstring>

#include "host_app.h"
#include "pluginterfaces/base/funknownimpl.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivstprocesscontext.h"
#include "pluginterfaces/vst/vsttypes.h"
#include "public.sdk/source/vst/hosting/processdata.h"
#include "public.sdk/source/vst/hosting/eventlist.h"
#include "public.sdk/source/vst/hosting/parameterchanges.h"

namespace miv {

using namespace Steinberg;

PluginLoader::PluginLoader() {
    host_app_ = owned(new HostApplication);
    component_handler_ = owned(new ComponentHandler);
}

PluginLoader::~PluginLoader() {
    unload();
}

bool PluginLoader::load(const std::string& plugin_path,
                         uint32_t sample_rate,
                         uint32_t block_size,
                         LoadedPluginInfo& info_out,
                         std::string& error_out) {
    sample_rate_ = sample_rate;
    block_size_ = block_size;

    // VST3 SDK の Module ヘルパで .vst3 をロード
    std::string load_err;
    module_ = VST3::Hosting::Module::create(plugin_path, load_err);
    if (!module_) {
        error_out = "Module::create failed: " + load_err;
        return false;
    }

    // Factory から最初の AudioProcessor クラスを探す
    const auto& factory = module_->getFactory();
    for (const auto& info : factory.classInfos()) {
        if (info.category() == kVstAudioEffectClass) {
            // IComponent をインスタンス化
            component_ = factory.createInstance<Vst::IComponent>(info.ID());
            if (!component_) {
                continue;
            }
            // ホストとして初期化
            if (component_->initialize(host_app_) != kResultOk) {
                component_ = nullptr;
                continue;
            }
            // IAudioProcessor を取得
            processor_ = Steinberg::FUnknownPtr<Vst::IAudioProcessor>(component_);
            if (!processor_) {
                component_->terminate();
                component_ = nullptr;
                continue;
            }
            // IEditController は同一クラスから来る場合もある
            controller_ = Steinberg::FUnknownPtr<Vst::IEditController>(component_);
            if (!controller_) {
                // 別クラスとして提供されることもある (= TUID で controller class を取って createInstance)
                TUID ctrl_cid;
                if (component_->getControllerClassId(ctrl_cid) == kResultOk) {
                    controller_ = factory.createInstance<Vst::IEditController>(VST3::UID::fromTUID(ctrl_cid));
                    if (controller_) {
                        controller_->initialize(host_app_);
                    }
                }
            }
            if (controller_) {
                controller_->setComponentHandler(component_handler_);
            }

            info_out.plugin_name = info.name();
            break;
        }
    }

    if (!component_ || !processor_) {
        error_out = "no AudioEffectClass found in plugin";
        unload();
        return false;
    }

    // Bus 設定: stereo input + stereo output に固定 (Phase 0)
    Vst::SpeakerArrangement arr_in = Vst::SpeakerArr::kStereo;
    Vst::SpeakerArrangement arr_out = Vst::SpeakerArr::kStereo;
    if (processor_->setBusArrangements(&arr_in, 1, &arr_out, 1) != kResultOk) {
        error_out = "setBusArrangements stereo/stereo failed";
        unload();
        return false;
    }
    component_->activateBus(Vst::kAudio, Vst::kInput, 0, true);
    component_->activateBus(Vst::kAudio, Vst::kOutput, 0, true);

    // ProcessSetup
    Vst::ProcessSetup setup{};
    setup.processMode = Vst::kRealtime;
    setup.symbolicSampleSize = Vst::kSample32;
    setup.maxSamplesPerBlock = static_cast<int32>(block_size);
    setup.sampleRate = static_cast<double>(sample_rate);
    if (processor_->setupProcessing(setup) != kResultOk) {
        error_out = "setupProcessing failed";
        unload();
        return false;
    }

    // Activate
    if (component_->setActive(true) != kResultOk) {
        error_out = "setActive(true) failed";
        unload();
        return false;
    }
    processor_->setProcessing(true);
    active_ = true;

    cached_latency_samples_ = static_cast<uint32_t>(processor_->getLatencySamples());
    info_out.latency_samples = cached_latency_samples_;

    // 事前確保: planar バッファ (各 channel あたり block_size sample)
    in_buffer_l_.resize(block_size);
    in_buffer_r_.resize(block_size);
    out_buffer_l_.resize(block_size);
    out_buffer_r_.resize(block_size);

    return true;
}

bool PluginLoader::process_block(const float* input, float* output, uint32_t num_frames) {
    if (!active_ || !processor_) return false;
    if (num_frames > block_size_) return false;

    // f32 packed stereo → planar に分解
    for (uint32_t i = 0; i < num_frames; ++i) {
        in_buffer_l_[i] = input[i * 2 + 0];
        in_buffer_r_[i] = input[i * 2 + 1];
    }

    // ProcessData セットアップ
    Vst::AudioBusBuffers in_bus{};
    Vst::AudioBusBuffers out_bus{};
    in_bus.numChannels = 2;
    out_bus.numChannels = 2;
    float* in_planar[2] = { in_buffer_l_.data(), in_buffer_r_.data() };
    float* out_planar[2] = { out_buffer_l_.data(), out_buffer_r_.data() };
    in_bus.channelBuffers32 = in_planar;
    out_bus.channelBuffers32 = out_planar;
    in_bus.silenceFlags = 0;
    out_bus.silenceFlags = 0;

    Vst::ProcessContext ctx{};
    ctx.state = Vst::ProcessContext::kPlaying;
    ctx.sampleRate = static_cast<double>(sample_rate_);
    ctx.tempo = 120.0;

    Vst::EventList input_events;
    Vst::EventList output_events;
    Vst::ParameterChanges input_params;
    Vst::ParameterChanges output_params;

    Vst::ProcessData data{};
    data.processMode = Vst::kRealtime;
    data.symbolicSampleSize = Vst::kSample32;
    data.numSamples = static_cast<int32>(num_frames);
    data.numInputs = 1;
    data.numOutputs = 1;
    data.inputs = &in_bus;
    data.outputs = &out_bus;
    data.inputParameterChanges = &input_params;
    data.outputParameterChanges = &output_params;
    data.inputEvents = &input_events;
    data.outputEvents = &output_events;
    data.processContext = &ctx;

    if (processor_->process(data) != kResultOk) {
        return false;
    }

    // planar → packed stereo に戻す
    for (uint32_t i = 0; i < num_frames; ++i) {
        output[i * 2 + 0] = out_buffer_l_[i];
        output[i * 2 + 1] = out_buffer_r_[i];
    }
    return true;
}

bool PluginLoader::get_gui_size(uint32_t& width_out, uint32_t& height_out) {
    if (!controller_) return false;
    // view_ が無ければ一時的に作って size だけ取って捨てるのが行儀よい。
    Steinberg::IPtr<Steinberg::IPlugView> v = view_;
    if (!v) {
        v = Steinberg::owned(controller_->createView(Steinberg::Vst::ViewType::kEditor));
        if (!v) return false;
    }
    Steinberg::ViewRect rect{};
    if (v->getSize(&rect) != Steinberg::kResultOk) {
        return false;
    }
    int32_t w = rect.right - rect.left;
    int32_t h = rect.bottom - rect.top;
    if (w <= 0 || h <= 0) return false;
    width_out = static_cast<uint32_t>(w);
    height_out = static_cast<uint32_t>(h);
    return true;
}

bool PluginLoader::show_gui(void* hwnd, std::string& error_out) {
    if (!controller_) {
        error_out = "controller not available";
        return false;
    }
    if (view_attached_) {
        // すでにアタッチ済みなら一度外して付け直す
        hide_gui();
    }
    if (!view_) {
        view_ = Steinberg::owned(controller_->createView(Steinberg::Vst::ViewType::kEditor));
        if (!view_) {
            error_out = "createView returned null (no editor)";
            return false;
        }
    }
    // VST3 の HWND タイプは "HWND" 文字列で指定 (kPlatformTypeHWND)
    if (view_->isPlatformTypeSupported(Steinberg::kPlatformTypeHWND) != Steinberg::kResultTrue) {
        error_out = "plugin view does not support HWND platform";
        view_ = nullptr;
        return false;
    }
    if (view_->attached(hwnd, Steinberg::kPlatformTypeHWND) != Steinberg::kResultOk) {
        error_out = "attached() failed";
        view_ = nullptr;
        return false;
    }
    view_attached_ = true;
    return true;
}

void PluginLoader::hide_gui() {
    if (view_attached_ && view_) {
        view_->removed();
    }
    view_attached_ = false;
    view_ = nullptr;
}

void PluginLoader::reset() {
    if (!processor_) return;
    // VST3 標準: setProcessing(false) → setProcessing(true) でフィルタ履歴 flush
    processor_->setProcessing(false);
    processor_->setProcessing(true);
}

void PluginLoader::unload() {
    // GUI が出ていれば先に外す (順序逆だとプラグインが crash することがある)
    hide_gui();
    if (active_ && processor_) {
        processor_->setProcessing(false);
    }
    if (active_ && component_) {
        component_->setActive(false);
    }
    active_ = false;
    if (controller_ && controller_ != Steinberg::FUnknownPtr<Vst::IEditController>(component_)) {
        controller_->terminate();
    }
    controller_ = nullptr;
    if (component_) {
        component_->terminate();
    }
    component_ = nullptr;
    processor_ = nullptr;
    module_.reset();
}

}  // namespace miv
