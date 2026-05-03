// VST3 プラグインの読み込みと音声処理ループ。
//
// VST3 SDK の `Steinberg::Vst::Hosting::Module` を使って .vst3 をロードし、
// 中の `IComponent` / `IAudioProcessor` / `IEditController` を取得する。
//
// 1 bridge プロセスにつき 1 プラグインのみ管理する。複数プラグインのチェーン化は
// Phase E で対応。

#pragma once

#include <cstdint>
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

struct PluginProbeInfo {
    std::string plugin_name;
    uint32_t audio_input_buses = 0;
    uint32_t audio_output_buses = 0;
    uint32_t event_input_buses = 0;
    uint32_t event_output_buses = 0;
    uint32_t audio_input_channels = 0;
    uint32_t audio_output_channels = 0;
    bool usable_audio_effect = false;
};

struct GuiWindowOptions {
    void* owner_hwnd = nullptr;
    uint32_t width = 0;
    uint32_t height = 0;
    bool resizable = true;
    bool has_initial_pos = false;
    int32_t x = 0;
    int32_t y = 0;
    std::string title;
};

class PluginLoader {
public:
    PluginLoader();
    ~PluginLoader();

    /// .vst3 を短時間だけロードし、mIV の音声処理に使える audio input/output を
    /// 持つか調べる。process setup / audio thread / GUI attach は行わない。
    static bool probe(const std::string& plugin_path,
                      PluginProbeInfo& info_out,
                      std::string& error_out);

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

    // reset() の後に呼ぶ delay-line silence fill。VST3 仕様上 setProcessing による
    // 内部状態 clear は "should" であり、すべての plugin が delay-line をクリアする
    // 保証はない。silence (= zeros) を `latency_samples` 分だけ process_block に流して、
    // delay-line を確実に silence で埋める。これによりシーク後の最初の real audio が
    // delay-line から押し出される silence の **後ろ** に並ぶので、pre-seek audio が
    // 漏れ出すことを防ぐ (= "belt-and-suspenders" な確実 flush)。
    // num_samples = 0 (= no latency) なら no-op。
    void flush_with_silence(uint32_t num_samples);

    // プラグインのアンロードと VST3 SDK のクリーンアップ。
    void unload();

    // 現在のプラグイン latency (= IAudioProcessor::getLatencySamples())。
    uint32_t latency_samples() const { return cached_latency_samples_; }

    /// プラグインから `restartComponent(kLatencyChanged)` が来ていないか polling し、
    /// 来ていれば最新値を取得して返す。main loop で 1 イテレーションに 1 回呼ぶ想定。
    /// - 戻り値 true: latency が変更されていた。`new_latency_out` に新値が入る。
    ///   呼び出し側はこれを親プロセスに `latency_changed` イベントで通知する。
    /// - 戻り値 false: 変更なし (= フラグが立っていなかった)。
    ///
    /// 内部: ComponentHandler のフラグを atomically consume してから、processor に
    /// `getLatencySamples()` を再問い合わせし、cached_latency_samples_ を更新する。
    bool poll_latency_change(uint32_t& new_latency_out);

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
    /// `resizable_out` には IPlugView::canResize() の結果を返す (= ホストが外側
    /// ウィンドウのリサイズ枠を出すかの判断材料)。
    bool query_gui_size_at_dpi(uint32_t dpi, uint32_t& width_out, uint32_t& height_out,
                                bool& resizable_out);
    /// 指定 HWND にプラグイン GUI をアタッチする。失敗時は false。
    bool show_gui(const GuiWindowOptions& options, bool visible, std::string& error_out);
    /// Already-attached GUI surface visibility toggle. Keeps the VST3 view
    /// attached and only hides/shows the bridge-owned top-level surface.
    void set_gui_visible(bool visible);
    /// Toggle topmost state for the bridge-owned plugin surface. The Rust side
    /// applies the same state to the host HWND so the two windows stay together.
    void set_gui_topmost(bool topmost);
    /// Update the owner HWND of the bridge-owned editor surface. Used when the
    /// active mIV viewport changes (main grid vs fullscreen viewport).
    void set_gui_owner(void* owner_hwnd);
    /// Relay mIV app activation to the bridge-owned plugin surface.
    void set_gui_app_active(bool active);
    void* gui_container_hwnd() const { return view_container_hwnd_; }
    /// Helpers for chain-level batched show/hide and z-order updates.
    void set_gui_surface_visible_state(bool visible);
    bool gui_surface_should_show() const;
    bool gui_surface_target_rect(int32_t& x_out, int32_t& y_out,
                                 int32_t& width_out, int32_t& height_out);
    void refresh_gui_surface_now();
    void handle_editor_window_size();
    void handle_editor_drag_start();
    void handle_editor_drag_tick(uint32_t msg);
    void handle_editor_drag_end();
    /// GUI を外す。HWND 自体は呼び出し側が破棄する。
    void hide_gui();

    /// ホストウィンドウのクライアント領域がユーザーリサイズされたことを
    /// プラグインに通知する。view->onSize(rect) を呼ぶ。
    void notify_host_resize(uint32_t width, uint32_t height);

    /// ユーザー drag による resize/move session が進行中かを設定する (Codex P4)。
    /// PlugFrame に伝搬し、session 中は `resizeView` の SetWindowPos を抑止する。
    void set_user_resizing(bool active);

    /// プラグイン内部状態 (= EQ カーブ等のパラメータ + chunk) を取得する。
    /// VST3 `IComponent::getState()` を `MemoryStream` 経由でバイト列に書き出す。
    /// 戻り値: true=成功 (= `out_bytes` に空でもないバイト列が入る)、false=失敗。
    bool query_state(std::vector<uint8_t>& out_bytes);

    /// プラグイン内部状態をバイト列から復元する (`IComponent::setState`)。
    /// `IEditController::setComponentState` も同期で呼んで UI 表示を合わせる。
    /// 安全のため一時的に `setProcessing(false)` してから復元、終わったら再有効化する。
    /// 戻り値: true=成功、false=失敗 (= 不正バイト列、setState 拒否)。
    bool restore_state(const std::vector<uint8_t>& bytes);

private:
    void refresh_gui_surface(void* container_hwnd);

    Steinberg::IPtr<HostApplication> host_app_;
    Steinberg::IPtr<ComponentHandler> component_handler_;
    Steinberg::IPtr<PlugFrame> plug_frame_;

    VST3::Hosting::Module::Ptr module_;
    Steinberg::IPtr<Steinberg::Vst::IComponent> component_;
    Steinberg::IPtr<Steinberg::Vst::IAudioProcessor> processor_;
    Steinberg::IPtr<Steinberg::Vst::IEditController> controller_;
    Steinberg::IPtr<Steinberg::IPlugView> view_;
    std::string plugin_name_;
    bool view_attached_ = false;
    void* view_host_hwnd_ = nullptr;
    void* view_container_hwnd_ = nullptr;
    bool gui_surface_visible_ = false;
    bool gui_app_active_ = true;
    uint32_t last_gui_width_ = 0;
    uint32_t last_gui_height_ = 0;

    uint32_t sample_rate_ = 0;
    uint32_t block_size_ = 0;
    uint32_t cached_latency_samples_ = 0;
    bool active_ = false;

    bool editor_drag_active_ = false;
    uint64_t editor_drag_started_ms_ = 0;
    uint64_t editor_drag_last_tick_ms_ = 0;
    uint32_t editor_drag_move_count_ = 0;
    uint32_t editor_drag_size_count_ = 0;
    uint32_t editor_drag_windowpos_count_ = 0;
    uint32_t editor_drag_max_gap_ms_ = 0;

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
