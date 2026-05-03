// VST3 IHostApplication 実装と関連サービス。
//
// VST3 プラグインが host に問い合わせる API (`IHostApplication::createInstance`,
// `IHostApplication::getName`) と、parameter 変更時のフィードバック先 (`IComponentHandler`)
// を最低限実装する。parameter 自動化や preset 管理は v0.10.0 ではスコープ外。

#pragma once

#include <atomic>
#include <mutex>
#include <unordered_map>
#include <utility>
#include <vector>

#include "pluginterfaces/base/funknown.h"
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/gui/iplugview.h"
#include "pluginterfaces/vst/ivsthostapplication.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstparameterchanges.h"
#include "pluginterfaces/vst/ivstpluginterfacesupport.h"
#include "public.sdk/source/vst/hosting/pluginterfacesupport.h"

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

    // FUnknown — IPlugInterfaceSupport を additional に返すため queryInterface を
    // カスタム実装する。addRef/release は標準 ref count。
    Steinberg::tresult PLUGIN_API queryInterface(const Steinberg::TUID _iid, void** obj) override;
    Steinberg::uint32 PLUGIN_API addRef() override;
    Steinberg::uint32 PLUGIN_API release() override;

private:
    Steinberg::int32 ref_count_ = 1;
    Steinberg::IPtr<Steinberg::Vst::PlugInterfaceSupport> plug_iface_support_;
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

    /// ホスト主導の `onSize` を送る直前に呼ぶ。ログ診断用に時刻を保持するが、
    /// `resizeView` の抑止判定は `user_resizing_` のみで行う。直近 WM_SIZE の
    /// 余韻だけでプラグイン内部 resize handle 由来の正規 resizeView を捨てると、
    /// editor が内部サイズだけ変わり、外枠と中身がずれる。
    void mark_user_resize();

    /// ユーザー drag によるリサイズ session の開始 / 終了 (Codex P4)。
    /// `active=true` 中は `resizeView` callback で SetWindowPos を **常に**
    /// スキップする。
    void set_user_resizing(bool active);

    Steinberg::tresult PLUGIN_API resizeView(Steinberg::IPlugView* view,
                                              Steinberg::ViewRect* newSize) override;

    DECLARE_FUNKNOWN_METHODS

private:
    void* host_hwnd_ = nullptr;
    /// 直前のホスト主導リサイズ (= notify_host_resize) のタイムスタンプ (ミリ秒)。
    /// resizeView コールバックが来た時刻と比較して、近い時刻なら "ユーザー drag
    /// による波及" とみなして SetWindowPos スキップ。0 = 未初期化。
    uint64_t last_user_resize_tick_ = 0;
    /// ユーザー drag による resize/move session 中フラグ (Codex P4)。
    /// host wndproc の WM_ENTERSIZEMOVE / WM_EXITSIZEMOVE で true/false が
    /// 設定される。true 中は resizeView の SetWindowPos を常にスキップする。
    bool user_resizing_ = false;
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

    /// プラグインが `restartComponent(kLatencyChanged)` を呼んだかをチェックして
    /// (= flag が立っていれば) クリアして true を返す。PluginLoader が main loop で
    /// polling して、true なら getLatencySamples() を再取得して mIV に通知する。
    bool consume_latency_changed_flag() {
        return latency_changed_pending_.exchange(false, std::memory_order_acq_rel);
    }

    DECLARE_FUNKNOWN_METHODS

private:
    std::mutex pending_mutex_;
    // 同一 ParamID の更新は last-write-wins で集約。
    // vector に push していると、UI 高速ドラッグ時に同 ParamID で複数値が
    // sampleOffset=0 に集中して積まれ、プラグインの補間器が振動 → クリックノイズ。
    std::unordered_map<Steinberg::Vst::ParamID, Steinberg::Vst::ParamValue> pending_changes_;
    /// `restartComponent(kLatencyChanged)` を受けて latency 再取得が必要かを示すフラグ。
    /// VST3 規約上、この通知は audio 処理を一旦止めて latency を再問い合わせろという指示。
    /// プラグインスレッド (any) から set、main thread から polling consume する。
    std::atomic<bool> latency_changed_pending_{false};
};

}  // namespace miv
