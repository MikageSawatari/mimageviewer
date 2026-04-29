// VST3 プラグインの読み込みと音声処理ループ。
//
// VST3 SDK の `Steinberg::Vst::Hosting::Module` を使って .vst3 をロードし、
// 中の `IComponent` / `IAudioProcessor` / `IEditController` を取得する。
//
// 1 bridge プロセスにつき 1 プラグインのみ管理する。複数プラグインのチェーン化は
// Phase E で対応。

#pragma once

#include <memory>
#include <string>
#include <vector>

#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstprocesscontext.h"
#include "public.sdk/source/vst/hosting/module.h"

namespace miv {

class HostApplication;
class ComponentHandler;

struct LoadedPluginInfo {
    std::string plugin_name;
    uint32_t latency_samples = 0;
    // params は今後拡張 (Phase A 後半で)
};

class PluginLoader {
public:
    PluginLoader();
    ~PluginLoader();

    // .vst3 ファイルをロードし、IAudioProcessor / IComponent をセットアップする。
    // sample_rate と block_size は ProcessSetup に渡される。
    // 失敗時 (= load 不可、初期化エラー) は std::nullopt を返す。
    bool load(const std::string& plugin_path,
              uint32_t sample_rate,
              uint32_t block_size,
              LoadedPluginInfo& info_out,
              std::string& error_out);

    // 1 ブロック分の音声を処理する。
    // input/output ともに f32 packed stereo (= [L, R, L, R, ...])、frame 数は block_size。
    // 返り値: true = 処理成功、false = エラー (= 状態異常、要 reset)
    bool process_block(const float* input, float* output, uint32_t num_frames);

    // 状態リセット (再生位置変更時等)。プラグイン内部のフィルタ履歴を flush する。
    void reset();

    // プラグインのアンロードと VST3 SDK のクリーンアップ。
    void unload();

    // 現在のプラグイン latency (= IAudioProcessor::getLatencySamples())。
    uint32_t latency_samples() const { return cached_latency_samples_; }

private:
    Steinberg::IPtr<HostApplication> host_app_;
    Steinberg::IPtr<ComponentHandler> component_handler_;

    VST3::Hosting::Module::Ptr module_;
    Steinberg::IPtr<Steinberg::Vst::IComponent> component_;
    Steinberg::IPtr<Steinberg::Vst::IAudioProcessor> processor_;
    Steinberg::IPtr<Steinberg::Vst::IEditController> controller_;

    uint32_t sample_rate_ = 0;
    uint32_t block_size_ = 0;
    uint32_t cached_latency_samples_ = 0;
    bool active_ = false;

    // process() に渡す ProcessData の事前確保バッファ (アロケーションを避けるため)
    std::vector<float> in_buffer_l_;
    std::vector<float> in_buffer_r_;
    std::vector<float> out_buffer_l_;
    std::vector<float> out_buffer_r_;
};

}  // namespace miv
