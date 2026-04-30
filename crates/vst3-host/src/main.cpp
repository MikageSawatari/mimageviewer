// mimageviewer-vst3-host: VST3 plugin host bridge
//
// 親 (Rust) プロセスから子プロセスとして起動され、stdin で制御メッセージを受け取り、
// stdout で応答 / 通知を返す。音声バッファは shared memory + named events で受け渡す。
//
// プロトコル詳細は include/protocol.h 参照。
//
// ## スレッド構成
//
// - **main thread**: 起動後すぐ GUI thread に変身 (= 自分が GUI スレッドになる)。
//   PeekMessage ループで Win32 メッセージを処理しつつ、コマンドキューを polling して
//   GUI 関連の VST3 操作 (createView / attached / removed / onSize 等) を実行する。
//   COM は STA (Single-Threaded Apartment) で初期化。
// - **stdin pump thread**: 親プロセスからのコマンドを読んで、コマンドキューに投入する
//   だけの専用スレッド。blocking read OK。
// - **audio thread**: 音声処理ループ (既存)。
//
// プラグインの GUI 子ウィンドウは bridge プロセスのいずれかのスレッドで作られるが、
// VST3 規約により attached を呼んだスレッド = main (GUI) thread で作成される。
// 子ウィンドウのメッセージは作成スレッドにディスパッチされるので、main thread の
// PeekMessage ループで処理される。これが無いと描画停止 (= 真っ白でハング)。

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <deque>
#include <iostream>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

#include <fcntl.h>
#include <io.h>
#include <objbase.h>  // CoInitializeEx
#include <windows.h>

#include "audio_pipe.h"
#include "host_app.h"
#include "plugin_loader.h"
#include "protocol.h"

namespace miv {

// stdin/stdout を binary モードに切り替える (Windows では \r\n 変換を抑止)。
static void setup_streams() {
    _setmode(_fileno(stdin), _O_BINARY);
    _setmode(_fileno(stdout), _O_BINARY);
    _setmode(_fileno(stderr), _O_BINARY);
}

// length-prefixed UTF-8 メッセージを読み出す。EOF で false。
static bool read_message(std::string& out) {
    uint32_t len = 0;
    if (std::fread(&len, sizeof(len), 1, stdin) != 1) {
        return false;
    }
    if (len == 0 || len > MAX_CONTROL_MSG_SIZE) {
        return false;
    }
    out.resize(len);
    if (std::fread(out.data(), 1, len, stdin) != len) {
        return false;
    }
    return true;
}

// stdout 書き込みは複数スレッド (main / audio thread) から呼ばれる可能性があるため
// mutex で保護する。
static std::mutex& stdout_mutex() {
    static std::mutex m;
    return m;
}

// length-prefixed UTF-8 メッセージを書き出して flush する。
static void write_message(const std::string& payload) {
    std::lock_guard<std::mutex> lk(stdout_mutex());
    uint32_t len = static_cast<uint32_t>(payload.size());
    std::fwrite(&len, sizeof(len), 1, stdout);
    std::fwrite(payload.data(), 1, payload.size(), stdout);
    std::fflush(stdout);
}

// minimum JSON エスケープ。protocol が単純なので外部依存ライブラリは使わない。
// payload は UTF-8、内部に " と \ のみエスケープする。
static std::string json_escape(const std::string& s) {
    std::string out;
    out.reserve(s.size() + 2);
    for (char c : s) {
        switch (c) {
            case '"':  out += "\\\""; break;
            case '\\': out += "\\\\"; break;
            case '\b': out += "\\b"; break;
            case '\f': out += "\\f"; break;
            case '\n': out += "\\n"; break;
            case '\r': out += "\\r"; break;
            case '\t': out += "\\t"; break;
            default:
                if (static_cast<unsigned char>(c) < 0x20) {
                    char buf[8];
                    std::snprintf(buf, sizeof(buf), "\\u%04x", static_cast<unsigned char>(c));
                    out += buf;
                } else {
                    out += c;
                }
        }
    }
    return out;
}

static void send_event_error(const std::string& detail) {
    std::string msg = "{\"event\":\"error\",\"detail\":\"" + json_escape(detail) + "\"}";
    write_message(msg);
}

class Bridge {
public:
    Bridge() = default;
    ~Bridge() = default;

    int run() {
        setup_streams();

        // VST3 GUI は STA を要求する。main thread (= GUI thread) で COM を初期化。
        HRESULT co_hr = CoInitializeEx(nullptr,
                                       COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);

        // stdin pump を別スレッドに分離。読んだコマンドはキューに投入する。
        running_ = true;
        std::thread stdin_thread([this]() {
            std::string msg;
            while (read_message(msg)) {
                {
                    std::lock_guard<std::mutex> lk(cmd_mutex_);
                    cmd_queue_.push_back(std::move(msg));
                }
                cmd_cv_.notify_all();
                msg.clear();
            }
            // EOF: 親が stdin を閉じた → graceful shutdown
            {
                std::lock_guard<std::mutex> lk(cmd_mutex_);
                running_ = false;
            }
            cmd_cv_.notify_all();
        });

        // メインスレッド: メッセージポンプ + コマンド処理ループ
        run_gui_loop();

        if (audio_thread_.joinable()) {
            audio_running_ = false;
            audio_thread_.join();
        }
        if (stdin_thread.joinable()) {
            // stdin はブロッキング read 中。fclose(stdin) で抜ける。
            // ただしユーザー側 (Rust) が stdin を閉じれば自然に EOF が来るので
            // 通常は join で待てる。安全のため数秒待ってデタッチでもよいが、
            // 単純化のため join。
            stdin_thread.detach();
        }

        if (loader_) {
            loader_->unload();
        }
        if (SUCCEEDED(co_hr)) {
            CoUninitialize();
        }
        return 0;
    }

private:
    // ── メインループ: メッセージポンプ + コマンドキュー処理 ──
    void run_gui_loop() {
        while (running_) {
            // 1) Win32 メッセージを全消化
            MSG msg;
            while (PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) {
                ::TranslateMessage(&msg);
                ::DispatchMessageW(&msg);
            }
            // 2) コマンドキューを 1 件処理
            std::string cmd_msg;
            {
                std::unique_lock<std::mutex> lk(cmd_mutex_);
                if (cmd_queue_.empty()) {
                    // メッセージが届いた瞬間に起こされるよう、PostThreadMessage を
                    // 使った方が厳密だが、シンプルに 8ms 程度のタイムアウトで
                    // PeekMessage と交互ポーリングする。
                    cmd_cv_.wait_for(lk, std::chrono::milliseconds(8));
                    continue;
                }
                cmd_msg = std::move(cmd_queue_.front());
                cmd_queue_.pop_front();
            }
            if (!handle_message(cmd_msg)) {
                running_ = false;
            }
        }
    }

    // 単純な JSON 解析: { "cmd": "<value>", ... } から cmd を取り出す。
    // POC 用なので本格的な JSON パーサは入れない (将来 nlohmann/json 採用検討)。
    static std::string extract_string_field(const std::string& json, const std::string& key) {
        std::string needle = "\"" + key + "\"";
        auto pos = json.find(needle);
        if (pos == std::string::npos) return {};
        pos = json.find(':', pos);
        if (pos == std::string::npos) return {};
        pos = json.find('"', pos);
        if (pos == std::string::npos) return {};
        auto end = json.find('"', pos + 1);
        if (end == std::string::npos) return {};
        return json.substr(pos + 1, end - pos - 1);
    }

    static uint64_t extract_number_field(const std::string& json, const std::string& key) {
        std::string needle = "\"" + key + "\"";
        auto pos = json.find(needle);
        if (pos == std::string::npos) return 0;
        pos = json.find(':', pos);
        if (pos == std::string::npos) return 0;
        ++pos;
        while (pos < json.size() && (json[pos] == ' ' || json[pos] == '\t')) ++pos;
        uint64_t v = 0;
        while (pos < json.size() && json[pos] >= '0' && json[pos] <= '9') {
            v = v * 10 + (json[pos] - '0');
            ++pos;
        }
        return v;
    }

    bool handle_message(const std::string& msg) {
        std::string cmd = extract_string_field(msg, "cmd");
        if (cmd == "hello") {
            std::string reply = "{\"event\":\"ready\",\"version\":" +
                                std::to_string(PROTOCOL_VERSION) + "}";
            write_message(reply);
            return true;
        }
        if (cmd == "open") {
            return handle_open(msg);
        }
        if (cmd == "reset") {
            if (loader_) loader_->reset();
            write_message("{\"event\":\"reset_done\"}");
            return true;
        }
        if (cmd == "query_gui_size") {
            // プラグインの推奨 GUI サイズを scale 込みで取得して返す。
            // bridge プロセスは Per-Monitor v2 Aware なので GetDpiForSystem は
            // primary monitor の DPI (= ユーザー環境では 144 等) を返す。
            if (!loader_) {
                send_event_error("query_gui_size: no plugin loaded");
                return true;
            }
            UINT dpi = GetDpiForSystem();
            if (dpi == 0) dpi = 96;
            uint32_t w = 0, h = 0;
            if (!loader_->query_gui_size_at_dpi(dpi, w, h)) {
                send_event_error("query_gui_size: getSize failed");
                return true;
            }
            std::string reply = "{\"event\":\"gui_size\",\"width\":" +
                                std::to_string(w) + ",\"height\":" +
                                std::to_string(h) + "}";
            write_message(reply);
            return true;
        }
        if (cmd == "show_gui") {
            if (!loader_) {
                send_event_error("show_gui: no plugin loaded");
                return true;
            }
            uint64_t hwnd_u = extract_number_field(msg, "hwnd");
            if (hwnd_u == 0) {
                send_event_error("show_gui: hwnd missing");
                return true;
            }
            std::string err;
            if (!loader_->show_gui(reinterpret_cast<void*>(hwnd_u), err)) {
                send_event_error("show_gui: " + err);
                return true;
            }
            uint32_t w = 0, h = 0;
            if (loader_->get_gui_size(w, h)) {
                std::string reply = "{\"event\":\"gui_attached\",\"width\":" +
                                    std::to_string(w) + ",\"height\":" +
                                    std::to_string(h) + "}";
                write_message(reply);
            } else {
                write_message("{\"event\":\"gui_attached\",\"width\":0,\"height\":0}");
            }
            return true;
        }
        if (cmd == "hide_gui") {
            if (loader_) loader_->hide_gui();
            write_message("{\"event\":\"gui_detached\"}");
            return true;
        }
        if (cmd == "notify_host_resize") {
            // host (tester) ウィンドウがユーザーリサイズされた → プラグインに通知して
            // 子ウィンドウを追従させる。応答は不要。
            if (!loader_) return true;
            uint32_t w = static_cast<uint32_t>(extract_number_field(msg, "width"));
            uint32_t h = static_cast<uint32_t>(extract_number_field(msg, "height"));
            if (w > 0 && h > 0) {
                loader_->notify_host_resize(w, h);
            }
            return true;
        }
        if (cmd == "close") {
            if (loader_) loader_->unload();
            loader_.reset();
            audio_running_ = false;
            if (audio_thread_.joinable()) audio_thread_.join();
            pipe_.detach();
            write_message("{\"event\":\"closed\"}");
            return true;
        }
        if (cmd == "shutdown") {
            return false;
        }
        send_event_error("unknown command: " + cmd);
        return true;
    }

    bool handle_open(const std::string& msg) {
        std::string plugin_path = extract_string_field(msg, "plugin_path");
        std::string shm_name = extract_string_field(msg, "shm_name");
        std::string sig_in_name = extract_string_field(msg, "sig_in");
        std::string sig_out_name = extract_string_field(msg, "sig_out");
        uint64_t shm_size = extract_number_field(msg, "shm_size");
        uint32_t sample_rate = static_cast<uint32_t>(extract_number_field(msg, "sample_rate"));
        uint32_t block_size = static_cast<uint32_t>(extract_number_field(msg, "block_size"));

        if (plugin_path.empty() || shm_name.empty() || sample_rate == 0 || block_size == 0) {
            send_event_error("missing required fields in open command");
            return true;
        }

        std::string err;
        if (!pipe_.attach(shm_name, shm_size, sig_in_name, sig_out_name, err)) {
            send_event_error("attach failed: " + err);
            return true;
        }

        loader_ = std::make_unique<PluginLoader>();
        LoadedPluginInfo info;
        if (!loader_->load(plugin_path, sample_rate, block_size, info, err)) {
            send_event_error("load failed: " + err);
            loader_.reset();
            pipe_.detach();
            return true;
        }

        std::string reply = "{\"event\":\"loaded\",\"plugin_name\":\"" +
                            json_escape(info.plugin_name) +
                            "\",\"latency_samples\":" +
                            std::to_string(info.latency_samples) + "}";
        write_message(reply);

        audio_running_ = true;
        audio_thread_ = std::thread(&Bridge::audio_loop, this, block_size);
        return true;
    }

    void audio_loop(uint32_t block_size) {
        const uint32_t channels = 2;
        std::vector<float> input(block_size * channels);
        std::vector<float> output(block_size * channels);

        std::fprintf(stderr, "[BRIDGE] audio_loop start (block=%u)\n", block_size);
        std::fflush(stderr);
        uint64_t blocks_in = 0, blocks_processed = 0, blocks_out = 0;
        uint64_t timeouts_in = 0, timeouts_out = 0;
        float input_peak = 0.0f, output_peak = 0.0f;
        auto last_report = std::chrono::steady_clock::now();
        auto report_now = [&]() {
            auto now = std::chrono::steady_clock::now();
            if (std::chrono::duration_cast<std::chrono::milliseconds>(now - last_report).count() < 1000) {
                return;
            }
            std::fprintf(stderr,
                "[BRIDGE] audio: in=%llu proc=%llu out=%llu to_in=%llu to_out=%llu in_peak=%.4f out_peak=%.4f\n",
                (unsigned long long)blocks_in,
                (unsigned long long)blocks_processed,
                (unsigned long long)blocks_out,
                (unsigned long long)timeouts_in,
                (unsigned long long)timeouts_out,
                input_peak, output_peak);
            std::fflush(stderr);
            blocks_in = blocks_processed = blocks_out = 0;
            timeouts_in = timeouts_out = 0;
            input_peak = output_peak = 0.0f;
            last_report = now;
        };

        while (audio_running_) {
            if (!pipe_.read_in(input.data(),
                                block_size * channels,
                                100 /* ms */)) {
                ++timeouts_in;
                report_now();
                if (!audio_running_) break;
                continue;
            }
            ++blocks_in;
            for (float v : input) input_peak = std::max(input_peak, std::fabs(v));

            if (loader_ && !loader_->process_block(input.data(), output.data(), block_size)) {
                send_event_error("process_block failed");
                audio_running_ = false;
                break;
            }
            ++blocks_processed;
            for (float v : output) output_peak = std::max(output_peak, std::fabs(v));

            if (!pipe_.write_out(output.data(),
                                  block_size * channels,
                                  100 /* ms */)) {
                ++timeouts_out;
                report_now();
                if (!audio_running_) break;
                continue;
            }
            ++blocks_out;
            report_now();
        }
        std::fprintf(stderr, "[BRIDGE] audio_loop exit\n");
        std::fflush(stderr);
    }

    std::unique_ptr<PluginLoader> loader_;
    AudioPipe pipe_;

    // GUI / コマンドキュー
    std::mutex cmd_mutex_;
    std::condition_variable cmd_cv_;
    std::deque<std::string> cmd_queue_;
    std::atomic<bool> running_{false};

    // audio
    std::thread audio_thread_;
    std::atomic<bool> audio_running_{false};
};

}  // namespace miv

int main(int argc, char** argv) {
    (void)argc;
    (void)argv;
    // bridge プロセスを Per-Monitor v2 DPI Aware に設定する。
    // これがないと GetDpiForSystem / GetDpiForWindow がプライマリ DPI ではなく
    // 96 を返してしまい、setContentScaleFactor で正しい scale を伝えられない。
    // VST3 GUI を任意のスレッドで attached する前に必ずプロセス全体に設定する必要がある。
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    miv::Bridge bridge;
    return bridge.run();
}
