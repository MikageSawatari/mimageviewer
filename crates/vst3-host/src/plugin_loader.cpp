// VST3 プラグインのロード & process loop 実装
//
// Phase 0 POC では「ロードしてパススルーで音を通す」のが目的。
// IComponent / IAudioProcessor / IEditController の取得と最低限の lifecycle 制御まで実装する。

#include "plugin_loader.h"

#include <algorithm>
#include <cstdio>
#include <cstring>
#include <vector>

#include <windows.h>

#include "pluginterfaces/gui/iplugviewcontentscalesupport.h"

namespace {
// stderr へのデバッグログ。tester 側で pipe して log_file に流す。
template <typename... Args>
void blog(const char* fmt, Args... args) {
    std::fprintf(stderr, "[BRIDGE] ");
    std::fprintf(stderr, fmt, args...);
    std::fprintf(stderr, "\n");
    std::fflush(stderr);
}
inline void blog(const char* msg) {
    std::fprintf(stderr, "[BRIDGE] %s\n", msg);
    std::fflush(stderr);
}
}  // namespace

#include "host_app.h"
#include "pluginterfaces/base/funknownimpl.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivstmessage.h"  // IConnectionPoint
#include "pluginterfaces/vst/ivstprocesscontext.h"
#include "pluginterfaces/vst/vsttypes.h"
#include "public.sdk/source/common/memorystream.h"
#include "public.sdk/source/vst/hosting/processdata.h"
#include "public.sdk/source/vst/hosting/eventlist.h"
#include "public.sdk/source/vst/hosting/parameterchanges.h"

namespace miv {

using namespace Steinberg;

PluginLoader::PluginLoader() {
    host_app_ = owned(new HostApplication);
    component_handler_ = owned(new ComponentHandler);
    plug_frame_ = owned(new PlugFrame);
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

                // VST3 必須: IComponent と IEditController を IConnectionPoint で
                // 接続し、component の state を controller に同期する。
                // これが無いと一部のプラグイン (Pro-Q 4 等) は UI 操作が音声処理に
                // 反映されず、内部アナライザも動作しない。
                auto component_cp = Steinberg::FUnknownPtr<Steinberg::Vst::IConnectionPoint>(component_);
                auto controller_cp = Steinberg::FUnknownPtr<Steinberg::Vst::IConnectionPoint>(controller_);
                if (component_cp && controller_cp) {
                    component_cp->connect(controller_cp);
                    controller_cp->connect(component_cp);
                    blog("load: component <-> controller connected");
                }

                // component の state を controller にコピー (= デフォルトパラメータ等)。
                Steinberg::MemoryStream stream;
                if (component_->getState(&stream) == Steinberg::kResultOk) {
                    stream.seek(0, Steinberg::IBStream::kIBSeekSet, nullptr);
                    if (controller_->setComponentState(&stream) == Steinberg::kResultOk) {
                        blog("load: setComponentState ok");
                    }
                }
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

    // Bus 設定: 動的に取得した bus 数に合わせて arrangement を渡す。
    // Pro-Q 4 等、サイドチェイン入力 bus を持つプラグインは 1 bus だけ
    // 渡すと kResultFalse を返すため、全 audio bus 分を埋める必要がある。
    //
    // 戦略:
    //   1) 全 bus を stereo で埋めて setBusArrangements
    //   2) 失敗したら副 bus を空 (= mono など最小) にしてリトライ
    //   3) それでも失敗すれば諦める
    int32 num_in_buses = component_->getBusCount(Vst::kAudio, Vst::kInput);
    int32 num_out_buses = component_->getBusCount(Vst::kAudio, Vst::kOutput);
    if (num_in_buses < 1 || num_out_buses < 1) {
        error_out = "plugin has no audio bus";
        unload();
        return false;
    }

    auto try_arrangements = [&](Vst::SpeakerArrangement aux) -> bool {
        std::vector<Vst::SpeakerArrangement> ins(num_in_buses, Vst::SpeakerArr::kStereo);
        std::vector<Vst::SpeakerArrangement> outs(num_out_buses, Vst::SpeakerArr::kStereo);
        for (int32 i = 1; i < num_in_buses; ++i) ins[i] = aux;
        for (int32 i = 1; i < num_out_buses; ++i) outs[i] = aux;
        return processor_->setBusArrangements(
                   ins.data(), num_in_buses, outs.data(), num_out_buses) == kResultOk;
    };

    bool arr_ok = try_arrangements(Vst::SpeakerArr::kStereo);
    if (!arr_ok) {
        // サイドチェインを mono で
        arr_ok = try_arrangements(Vst::SpeakerArr::kMono);
    }
    if (!arr_ok) {
        // サイドチェインを空 (= 無効化) で
        arr_ok = try_arrangements(Vst::SpeakerArr::kEmpty);
    }
    if (!arr_ok) {
        error_out = "setBusArrangements failed for stereo main bus (in="
                    + std::to_string(num_in_buses) + " out="
                    + std::to_string(num_out_buses) + ")";
        unload();
        return false;
    }

    // main bus (index 0) のみ active にし、副 bus は無効にして処理経路から外す
    for (int32 i = 0; i < num_in_buses; ++i) {
        component_->activateBus(Vst::kAudio, Vst::kInput, i, i == 0);
    }
    for (int32 i = 0; i < num_out_buses; ++i) {
        component_->activateBus(Vst::kAudio, Vst::kOutput, i, i == 0);
    }

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
    // 副 bus 用 silence (サイドチェインに無音を流すため)
    dummy_in_buf_.assign(block_size, 0.0f);
    dummy_out_buf_.assign(block_size, 0.0f);
    // process_block で参照する bus 数
    num_in_buses_ = num_in_buses;
    num_out_buses_ = num_out_buses;

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

    // ProcessData セットアップ — VST3 仕様により ProcessData::numInputs/numOutputs
    // は **getBusCount で得た値と一致** させる必要がある。Pro-Q 4 等サイドチェイン
    // 入力 bus を持つプラグインに 1 個だけ渡すと UB → 音声が届いていないように
    // 見える原因になる。
    std::vector<Vst::AudioBusBuffers> in_buses(num_in_buses_);
    std::vector<Vst::AudioBusBuffers> out_buses(num_out_buses_);

    // main bus
    float* main_in_planar[2] = { in_buffer_l_.data(), in_buffer_r_.data() };
    float* main_out_planar[2] = { out_buffer_l_.data(), out_buffer_r_.data() };
    in_buses[0].numChannels = 2;
    in_buses[0].channelBuffers32 = main_in_planar;
    in_buses[0].silenceFlags = 0;
    out_buses[0].numChannels = 2;
    out_buses[0].channelBuffers32 = main_out_planar;
    out_buses[0].silenceFlags = 0;

    // 副 bus (サイドチェイン等): silence buffer + silenceFlags 全立て
    float* dummy_in_planar[2] = { dummy_in_buf_.data(), dummy_in_buf_.data() };
    float* dummy_out_planar[2] = { dummy_out_buf_.data(), dummy_out_buf_.data() };
    for (int32 i = 1; i < num_in_buses_; ++i) {
        in_buses[i].numChannels = 2;
        in_buses[i].channelBuffers32 = dummy_in_planar;
        in_buses[i].silenceFlags = 0x3;  // ch0+ch1 silent
    }
    for (int32 i = 1; i < num_out_buses_; ++i) {
        out_buses[i].numChannels = 2;
        out_buses[i].channelBuffers32 = dummy_out_planar;
        out_buses[i].silenceFlags = 0x3;
    }

    Vst::ProcessContext ctx{};
    // 時間進行を伝えるフラグも立てる。立てないと Pro-Q 4 のアナライザ等は
    // 時刻情報を信用せずアナライザ更新を停止する。
    ctx.state = Vst::ProcessContext::kPlaying
              | Vst::ProcessContext::kContTimeValid
              | Vst::ProcessContext::kProjectTimeMusicValid
              | Vst::ProcessContext::kTempoValid;
    ctx.sampleRate = static_cast<double>(sample_rate_);
    ctx.projectTimeSamples = process_time_samples_;
    ctx.continousTimeSamples = process_time_samples_;
    ctx.projectTimeMusic = static_cast<double>(process_time_samples_) /
                            static_cast<double>(sample_rate_) *
                            (120.0 / 60.0); // 120 BPM 想定の quarter note 数
    ctx.tempo = 120.0;
    process_time_samples_ += static_cast<int64_t>(num_frames);

    Vst::EventList input_events;
    Vst::EventList output_events;
    Vst::ParameterChanges input_params;
    Vst::ParameterChanges output_params;

    Vst::ProcessData data{};
    data.processMode = Vst::kRealtime;
    data.symbolicSampleSize = Vst::kSample32;
    data.numSamples = static_cast<int32>(num_frames);
    data.numInputs = num_in_buses_;
    data.numOutputs = num_out_buses_;
    data.inputs = in_buses.data();
    data.outputs = out_buses.data();
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
    // 既存 view_ から純粋に getSize する (= scale 設定を変更しない)。
    // 既に show_gui で setContentScaleFactor 済みであれば、その scale 込みの
    // 物理ピクセル値を返す。
    // view_ が無ければ一時 view を作って素のサイズを取得 (scale 1.0 想定)。
    if (!controller_) return false;
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

bool PluginLoader::query_gui_size_at_dpi(uint32_t dpi, uint32_t& width_out, uint32_t& height_out) {
    // 一時 view を作り、指定 DPI の scale を伝えてから getSize → 破棄。
    // ホストウィンドウを作る前に正しいサイズを知るためのクエリ用。
    if (!controller_) return false;
    auto v = Steinberg::owned(controller_->createView(Steinberg::Vst::ViewType::kEditor));
    if (!v) return false;
    Steinberg::FUnknownPtr<Steinberg::IPlugViewContentScaleSupport> css(v);
    if (css) {
        if (dpi == 0) dpi = 96;
        float factor = static_cast<float>(dpi) / 96.0f;
        css->setContentScaleFactor(factor);
        blog("query_gui_size_at_dpi: setContentScaleFactor=%.3f (dpi=%u)", factor, dpi);
    }
    Steinberg::ViewRect rect{};
    if (v->getSize(&rect) != Steinberg::kResultOk) return false;
    int32_t w = rect.right - rect.left;
    int32_t h = rect.bottom - rect.top;
    if (w <= 0 || h <= 0) return false;
    width_out = static_cast<uint32_t>(w);
    height_out = static_cast<uint32_t>(h);
    return true;
}

bool PluginLoader::show_gui(void* hwnd, std::string& error_out) {
    blog("show_gui start hwnd=0x%llx", (unsigned long long)hwnd);
    if (!controller_) {
        error_out = "controller not available";
        return false;
    }
    if (view_attached_) {
        // すでにアタッチ済みなら一度外して付け直す
        blog("show_gui: already attached, hiding first");
        hide_gui();
    }
    if (!view_) {
        blog("show_gui: createView(kEditor)");
        view_ = Steinberg::owned(controller_->createView(Steinberg::Vst::ViewType::kEditor));
        if (!view_) {
            error_out = "createView returned null (no editor)";
            return false;
        }
        blog("show_gui: createView ok");
    }
    // VST3 の HWND タイプは "HWND" 文字列で指定 (kPlatformTypeHWND)
    blog("show_gui: isPlatformTypeSupported(HWND)");
    if (view_->isPlatformTypeSupported(Steinberg::kPlatformTypeHWND) != Steinberg::kResultTrue) {
        error_out = "plugin view does not support HWND platform";
        view_ = nullptr;
        return false;
    }
    blog("show_gui: setFrame");
    // attached より **前に** setFrame を呼ぶ。Pro-Q 4 等多くのプラグインは
    // frame が無いと描画開始しない (= 真っ白でハング)。
    plug_frame_->set_host_hwnd(hwnd);
    view_->setFrame(plug_frame_);

    // DPI scale をプラグインに伝える。これが無いとプラグインは "100% 想定" で
    // 描画してしまい、Per-Monitor v2 環境で位置/サイズがずれる
    // (Pro-Q 4 で「右下しか見えない」現象の原因)。
    Steinberg::FUnknownPtr<Steinberg::IPlugViewContentScaleSupport> css(view_);
    if (css) {
        UINT dpi = GetDpiForWindow(reinterpret_cast<HWND>(hwnd));
        if (dpi == 0) dpi = GetDpiForSystem();
        if (dpi == 0) dpi = 96;
        float factor = static_cast<float>(dpi) / 96.0f;
        css->setContentScaleFactor(factor);
        blog("show_gui: setContentScaleFactor=%.3f (dpi=%u)", factor, dpi);
    } else {
        blog("show_gui: plugin does not implement IPlugViewContentScaleSupport");
    }

    blog("show_gui: attached(hwnd, HWND)");
    if (view_->attached(hwnd, Steinberg::kPlatformTypeHWND) != Steinberg::kResultOk) {
        error_out = "attached() failed";
        view_->setFrame(nullptr);
        view_ = nullptr;
        return false;
    }
    view_attached_ = true;
    blog("show_gui: attached ok");

    // attached 後に推奨サイズで onSize を呼んで「このサイズで描画して」と通知する。
    // これも描画開始トリガとして必要なプラグインが多い。
    Steinberg::ViewRect rect{};
    if (view_->getSize(&rect) == Steinberg::kResultOk) {
        blog("show_gui: getSize=%dx%d, onSize",
             rect.right - rect.left, rect.bottom - rect.top);
        view_->onSize(&rect);
        blog("show_gui: onSize done");
    } else {
        blog("show_gui: getSize failed");
    }
    blog("show_gui done");
    return true;
}

void PluginLoader::notify_host_resize(uint32_t width, uint32_t height) {
    if (!view_attached_ || !view_) return;
    Steinberg::ViewRect rect{0, 0,
                             static_cast<Steinberg::int32>(width),
                             static_cast<Steinberg::int32>(height)};
    view_->onSize(&rect);
}

void PluginLoader::hide_gui() {
    if (view_attached_ && view_) {
        view_->removed();
        view_->setFrame(nullptr);
    }
    view_attached_ = false;
    view_ = nullptr;
}

void PluginLoader::reset() {
    if (!processor_) return;
    // VST3 標準: setProcessing(false) → setProcessing(true) でフィルタ履歴 flush
    processor_->setProcessing(false);
    processor_->setProcessing(true);
    // 時刻もリセット (= ProcessContext の sample カウンタ)
    process_time_samples_ = 0;
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
    // ConnectionPoint を切断 (load で connect した分)
    if (component_ && controller_) {
        auto comp_cp = Steinberg::FUnknownPtr<Vst::IConnectionPoint>(component_);
        auto ctrl_cp = Steinberg::FUnknownPtr<Vst::IConnectionPoint>(controller_);
        if (comp_cp && ctrl_cp) {
            comp_cp->disconnect(ctrl_cp);
            ctrl_cp->disconnect(comp_cp);
        }
    }
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
