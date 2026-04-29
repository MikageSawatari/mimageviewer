// mimageviewer-vst3-host: VST3 plugin host bridge
//
// 親 (Rust) プロセスから子プロセスとして起動され、stdin で制御メッセージを受け取り、
// stdout で応答 / 通知を返す。音声バッファは shared memory + named events で受け渡す。
//
// プロトコル詳細は include/protocol.h 参照。

#include <atomic>
#include <cstdint>
#include <cstdio>
#include <iostream>
#include <string>
#include <thread>

#include <fcntl.h>
#include <io.h>
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

// length-prefixed UTF-8 メッセージを書き出して flush する。
static void write_message(const std::string& payload) {
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

        std::string msg;
        while (read_message(msg)) {
            if (!handle_message(msg)) {
                // shutdown 命令 or 致命的エラー
                break;
            }
        }
        // 親が stdin を閉じたら graceful shutdown
        if (loader_) {
            loader_->unload();
        }
        return 0;
    }

private:
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
            // 親に ready を返す。shm 名は親が "open" 段階で渡してくる仕様。
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
        if (cmd == "show_gui") {
            if (!loader_) {
                send_event_error("show_gui: no plugin loaded");
                return true;
            }
            // hwnd は u64 (= ポインタ値) で渡される
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
            // 推奨サイズを通知
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
        if (cmd == "close") {
            if (loader_) loader_->unload();
            loader_.reset();
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

        // shared memory アタッチ
        std::string err;
        if (!pipe_.attach(shm_name, shm_size, sig_in_name, sig_out_name, err)) {
            send_event_error("attach failed: " + err);
            return true;
        }

        // VST3 プラグインロード
        loader_ = std::make_unique<PluginLoader>();
        LoadedPluginInfo info;
        if (!loader_->load(plugin_path, sample_rate, block_size, info, err)) {
            send_event_error("load failed: " + err);
            loader_.reset();
            pipe_.detach();
            return true;
        }

        // ロード成功通知
        std::string reply = "{\"event\":\"loaded\",\"plugin_name\":\"" +
                            json_escape(info.plugin_name) +
                            "\",\"latency_samples\":" +
                            std::to_string(info.latency_samples) + "}";
        write_message(reply);

        // 音声処理スレッド開始
        running_ = true;
        audio_thread_ = std::thread(&Bridge::audio_loop, this, block_size);
        return true;
    }

    void audio_loop(uint32_t block_size) {
        // POC 用の最小ループ: 親 → in_ring → process → out_ring → 親
        // チャンネル数は stereo 固定 (= 2)
        const uint32_t channels = 2;
        std::vector<float> input(block_size * channels);
        std::vector<float> output(block_size * channels);

        while (running_) {
            // 親がデータを書くまで待機 (timeout 付き、graceful shutdown 対応)
            if (!pipe_.read_in(input.data(),
                                block_size * channels,
                                100 /* ms */)) {
                if (!running_) break;
                continue;  // タイムアウトはノーマル (再生停止中等)
            }
            // プラグインで処理
            if (loader_ && !loader_->process_block(input.data(), output.data(), block_size)) {
                send_event_error("process_block failed");
                running_ = false;
                break;
            }
            // 親に書き戻し
            if (!pipe_.write_out(output.data(),
                                  block_size * channels,
                                  100 /* ms */)) {
                if (!running_) break;
                continue;
            }
        }
    }

    PluginLoader* loader_ptr() { return loader_.get(); }

    std::unique_ptr<PluginLoader> loader_;
    AudioPipe pipe_;
    std::thread audio_thread_;
    std::atomic<bool> running_{false};
};

}  // namespace miv

int main(int argc, char** argv) {
    (void)argc;
    (void)argv;
    miv::Bridge bridge;
    return bridge.run();
}
