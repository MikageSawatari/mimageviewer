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

#include "host_app.h"  // PlugFrame, HostApplication, ComponentHandler の完全型
#include "pluginterfaces/gui/iplugview.h"
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

    // ── GUI (IPlugView) 制御 ──
    //
    // VST3 ではプラグインの GUI は別オブジェクト (`IPlugView`) として提供され、
    // ホストが用意した親 HWND に `attached()` で取り付ける。
    // tester では Win32 で空ウィンドウを 1 つ作り、その HWND を渡す方式。

    /// プラグインの推奨 GUI サイズを取得する。
    /// 既に attached された view_ がある場合は scale 設定を変更せず getSize する
    /// (= show_gui 後の正しいサイズ)。view_ が無ければ一時 view を作って scale 1.0
    /// 想定で getSize する。返り値 false = エディター無し。
    bool get_gui_size(uint32_t& width_out, uint32_t& height_out);

    /// 指定 DPI で setContentScaleFactor してからプラグイン推奨サイズを取得する。
    /// ホストウィンドウを作る前のサイズクエリ用 (一時 view を使い捨て)。
    bool query_gui_size_at_dpi(uint32_t dpi, uint32_t& width_out, uint32_t& height_out);
    /// 指定 HWND にプラグイン GUI をアタッチする。失敗時は false。
    bool show_gui(void* hwnd, std::string& error_out);
    /// GUI を外す。HWND 自体は呼び出し側が破棄する。
    void hide_gui();

    /// ホストウィンドウのクライアント領域がユーザーリサイズされたことを
    /// プラグインに通知する。view->onSize(rect) を呼ぶ。
    void notify_host_resize(uint32_t width, uint32_t height);

private:
    Steinberg::IPtr<HostApplication> host_app_;
    Steinberg::IPtr<ComponentHandler> component_handler_;
    Steinberg::IPtr<PlugFrame> plug_frame_;

    VST3::Hosting::Module::Ptr module_;
    Steinberg::IPtr<Steinberg::Vst::IComponent> component_;
    Steinberg::IPtr<Steinberg::Vst::IAudioProcessor> processor_;
    Steinberg::IPtr<Steinberg::Vst::IEditController> controller_;
    Steinberg::IPtr<Steinberg::IPlugView> view_;
    bool view_attached_ = false;

    uint32_t sample_rate_ = 0;
    uint32_t block_size_ = 0;
    uint32_t cached_latency_samples_ = 0;
    bool active_ = false;

    // bus 数 (load 時に取得)。Pro-Q 4 等のサイドチェイン入力プラグインは
    // num_in_buses_ = 2 になる。ProcessData::numInputs はこの値と一致する必要がある。
    int32_t num_in_buses_ = 0;
    int32_t num_out_buses_ = 0;

    // ProcessContext の時刻フィールド (sample 単位)。process_block ごとに進める。
    // 進めないと Pro-Q 4 等のアナライザ系は「時間が止まっている」と判断して
    // 更新を停止する。
    int64_t process_time_samples_ = 0;

    // process() に渡す ProcessData の事前確保バッファ (アロケーションを避けるため)
    std::vector<float> in_buffer_l_;
    std::vector<float> in_buffer_r_;
    std::vector<float> out_buffer_l_;
    std::vector<float> out_buffer_r_;
    // 副 bus 用 silence buffer (= サイドチェイン input/output 用、無音固定)
    std::vector<float> dummy_in_buf_;
    std::vector<float> dummy_out_buf_;
};

}  // namespace miv
