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
#include <cstring>  // memcpy
#include <deque>
#include <iostream>
#include <memory>
#include <mutex>
#include <cstdlib>
#include <string>
#include <thread>
#include <vector>

#include <xmmintrin.h> // _MM_SET_FLUSH_ZERO_MODE
#include <pmmintrin.h> // _MM_SET_DENORMALS_ZERO_MODE

#include <fcntl.h>
#include <io.h>
#include <objbase.h>  // CoInitializeEx
#include <windows.h>
#include <avrt.h>      // AvSetMmThreadCharacteristicsW (MMCSS) — windows.h の後

#include "audio_pipe.h"
#include "host_app.h"
#include "plugin_loader.h"
#include "protocol.h"

namespace miv {

enum class BridgeMainState : int {
    Starting = 0,
    Idle,
    PumpingMessages,
    PollingLatency,
    WaitingCmdQueue,
    ProcessingCmd,
    ShuttingDown,
    Stopped,
};

enum class BridgeReaderState : int {
    Starting = 0,
    WaitingRead,
    Queueing,
    Eof,
    Stopped,
};

static const char* bridge_main_state_name(int state) {
    switch (static_cast<BridgeMainState>(state)) {
    case BridgeMainState::Starting: return "Starting";
    case BridgeMainState::Idle: return "Idle";
    case BridgeMainState::PumpingMessages: return "PumpingMessages";
    case BridgeMainState::PollingLatency: return "PollingLatency";
    case BridgeMainState::WaitingCmdQueue: return "WaitingCmdQueue";
    case BridgeMainState::ProcessingCmd: return "ProcessingCmd";
    case BridgeMainState::ShuttingDown: return "ShuttingDown";
    case BridgeMainState::Stopped: return "Stopped";
    default: return "Unknown";
    }
}

static const char* bridge_reader_state_name(int state) {
    switch (static_cast<BridgeReaderState>(state)) {
    case BridgeReaderState::Starting: return "Starting";
    case BridgeReaderState::WaitingRead: return "WaitingRead";
    case BridgeReaderState::Queueing: return "Queueing";
    case BridgeReaderState::Eof: return "Eof";
    case BridgeReaderState::Stopped: return "Stopped";
    default: return "Unknown";
    }
}

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

void send_event_error(const std::string& detail) {
    std::string msg = "{\"event\":\"error\",\"detail\":\"" + json_escape(detail) + "\"}";
    write_message(msg);
}

void send_event_gui_user_hidden(uint64_t slot_id) {
    std::string msg = "{\"event\":\"gui_user_hidden\",\"slot_id\":" +
                      std::to_string(slot_id) + "}";
    write_message(msg);
}

static std::string wide_to_utf8(const wchar_t* text) {
    if (!text || !*text) {
        return {};
    }
    int needed = WideCharToMultiByte(CP_UTF8, 0, text, -1, nullptr, 0, nullptr, nullptr);
    if (needed <= 1) {
        return {};
    }
    std::string out(static_cast<size_t>(needed - 1), '\0');
    WideCharToMultiByte(CP_UTF8, 0, text, -1, out.data(), needed, nullptr, nullptr);
    return out;
}

static BOOL CALLBACK enum_current_thread_window_proc(HWND hwnd, LPARAM param) {
    const char* label = reinterpret_cast<const char*>(param);
    wchar_t class_name[256] = {};
    wchar_t title[256] = {};
    GetClassNameW(hwnd, class_name, 256);
    GetWindowTextW(hwnd, title, 256);
    std::fprintf(stderr,
                 "[BRIDGE] main thread window %s hwnd=0x%llx class=\"%s\" title=\"%s\"\n",
                 label ? label : "(unknown)",
                 static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(hwnd)),
                 wide_to_utf8(class_name).c_str(),
                 wide_to_utf8(title).c_str());
    return TRUE;
}

static void debug_dump_current_thread_windows(const char* label) {
    EnumThreadWindows(GetCurrentThreadId(),
                      enum_current_thread_window_proc,
                      reinterpret_cast<LPARAM>(label));
    std::fflush(stderr);
}

// プラグイン内部状態を IPC で送るための base64 encode/decode (RFC 4648)。
// 外部依存ライブラリを増やしたくないので最小実装。state チャンクは典型
// 数 KB - 数十 KB 規模なので速度より簡潔さを優先する。
static const char kB64Alphabet[] =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

static std::string base64_encode(const std::vector<uint8_t>& bytes) {
    std::string out;
    out.reserve(((bytes.size() + 2) / 3) * 4);
    size_t i = 0;
    while (i + 3 <= bytes.size()) {
        uint32_t v = (uint32_t(bytes[i]) << 16) |
                     (uint32_t(bytes[i + 1]) << 8) |
                     uint32_t(bytes[i + 2]);
        out.push_back(kB64Alphabet[(v >> 18) & 0x3F]);
        out.push_back(kB64Alphabet[(v >> 12) & 0x3F]);
        out.push_back(kB64Alphabet[(v >> 6) & 0x3F]);
        out.push_back(kB64Alphabet[v & 0x3F]);
        i += 3;
    }
    if (i < bytes.size()) {
        uint32_t v = uint32_t(bytes[i]) << 16;
        size_t rem = bytes.size() - i;
        if (rem == 2) v |= uint32_t(bytes[i + 1]) << 8;
        out.push_back(kB64Alphabet[(v >> 18) & 0x3F]);
        out.push_back(kB64Alphabet[(v >> 12) & 0x3F]);
        out.push_back(rem == 2 ? kB64Alphabet[(v >> 6) & 0x3F] : '=');
        out.push_back('=');
    }
    return out;
}

static int base64_lookup(char c) {
    if (c >= 'A' && c <= 'Z') return c - 'A';
    if (c >= 'a' && c <= 'z') return c - 'a' + 26;
    if (c >= '0' && c <= '9') return c - '0' + 52;
    if (c == '+') return 62;
    if (c == '/') return 63;
    return -1;
}

static bool base64_decode(const std::string& in, std::vector<uint8_t>& out) {
    out.clear();
    out.reserve((in.size() / 4) * 3);
    uint32_t buf = 0;
    int bits = 0;
    for (char c : in) {
        if (c == '=' || c == '\n' || c == '\r' || c == ' ' || c == '\t') {
            continue;
        }
        int v = base64_lookup(c);
        if (v < 0) {
            out.clear();
            return false;
        }
        buf = (buf << 6) | static_cast<uint32_t>(v);
        bits += 6;
        if (bits >= 8) {
            bits -= 8;
            out.push_back(static_cast<uint8_t>((buf >> bits) & 0xFF));
        }
    }
    return true;
}

struct ChainSnapshot {
    std::vector<uint64_t> slot_ids;
    std::vector<PluginLoader*> loaders;
    std::vector<bool> bypassed;
};

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
        enter_main_state(BridgeMainState::Starting);
        enter_reader_state(BridgeReaderState::Starting);
        std::thread watchdog_thread(&Bridge::watchdog_loop, this);
        std::thread stdin_thread([this]() {
            std::string msg;
            enter_reader_state(BridgeReaderState::WaitingRead);
            while (running_ && read_message(msg)) {
                reader_cmds_received_.fetch_add(1, std::memory_order_relaxed);
                enter_reader_state(BridgeReaderState::Queueing);
                {
                    std::lock_guard<std::mutex> lk(cmd_mutex_);
                    cmd_queue_.push_back(std::move(msg));
                }
                cmd_cv_.notify_all();
                msg.clear();
                enter_reader_state(BridgeReaderState::WaitingRead);
            }
            enter_reader_state(BridgeReaderState::Eof);
            // EOF: 親が stdin を閉じた → graceful shutdown
            {
                std::lock_guard<std::mutex> lk(cmd_mutex_);
                running_ = false;
            }
            cmd_cv_.notify_all();
            enter_reader_state(BridgeReaderState::Stopped);
        });

        // メインスレッド: メッセージポンプ + コマンド処理ループ
        run_gui_loop();
        enter_main_state(BridgeMainState::ShuttingDown);
        running_ = false;
        cmd_cv_.notify_all();
        if (watchdog_thread.joinable()) {
            watchdog_thread.join();
        }

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

        {
            std::lock_guard<std::mutex> lk(loaders_mutex_);
            processing_order_.clear();
            plugin_bypass_.clear();
            rebuild_chain_snapshot_unlocked();
            if (loader_) {
                loader_->unload();
            }
            for (auto& loader : extra_loaders_) {
                if (loader) {
                    loader->unload();
                }
            }
        }
        if (SUCCEEDED(co_hr)) {
            CoUninitialize();
        }
        return 0;
    }

private:
    void enter_main_state(BridgeMainState state) {
        main_state_entered_tick_.store(GetTickCount64(), std::memory_order_release);
        main_state_.store(static_cast<int>(state), std::memory_order_release);
    }

    void enter_reader_state(BridgeReaderState state) {
        reader_state_entered_tick_.store(GetTickCount64(), std::memory_order_release);
        reader_state_.store(static_cast<int>(state), std::memory_order_release);
    }

    void set_current_cmd(std::string cmd) {
        std::lock_guard<std::mutex> lk(state_mutex_);
        current_cmd_ = std::move(cmd);
    }

    void watchdog_loop() {
        while (running_) {
            ::Sleep(1000);
            const ULONGLONG now = GetTickCount64();
            const int main_state = main_state_.load(std::memory_order_acquire);
            const int reader_state = reader_state_.load(std::memory_order_acquire);
            const ULONGLONG main_entered =
                main_state_entered_tick_.load(std::memory_order_acquire);
            const ULONGLONG reader_entered =
                reader_state_entered_tick_.load(std::memory_order_acquire);

            size_t queue_size = 0;
            bool queue_size_known = false;
            if (cmd_mutex_.try_lock()) {
                queue_size = cmd_queue_.size();
                cmd_mutex_.unlock();
                queue_size_known = true;
            }

            std::string current_cmd;
            if (state_mutex_.try_lock()) {
                current_cmd = current_cmd_;
                state_mutex_.unlock();
            } else {
                current_cmd = "<locked>";
            }
            if (current_cmd.empty()) {
                current_cmd = "-";
            }
            const UINT dispatch_msg = main_dispatch_msg_.load(std::memory_order_acquire);
            const uintptr_t dispatch_hwnd = main_dispatch_hwnd_.load(std::memory_order_acquire);
            const ULONGLONG dispatch_started =
                main_dispatch_started_tick_.load(std::memory_order_acquire);
            const ULONGLONG dispatch_in_ms =
                dispatch_started == 0 ? 0 : now - dispatch_started;

            if (queue_size_known) {
                std::fprintf(stderr,
                             "[BRIDGE main heartbeat] state=%s in_state_ms=%llu current_cmd=%s "
                             "dispatch_msg=0x%X dispatch_hwnd=0x%llx dispatch_in_ms=%llu "
                             "reader_state=%s reader_in_state_ms=%llu queue_size=%zu "
                             "cmds_received=%llu cmds_processed=%llu\n",
                             bridge_main_state_name(main_state),
                             static_cast<unsigned long long>(now - main_entered),
                             current_cmd.c_str(),
                             dispatch_msg,
                             static_cast<unsigned long long>(dispatch_hwnd),
                             static_cast<unsigned long long>(dispatch_in_ms),
                             bridge_reader_state_name(reader_state),
                             static_cast<unsigned long long>(now - reader_entered),
                             queue_size,
                             static_cast<unsigned long long>(
                                 reader_cmds_received_.load(std::memory_order_relaxed)),
                             static_cast<unsigned long long>(
                                 main_cmds_processed_.load(std::memory_order_relaxed)));
            } else {
                std::fprintf(stderr,
                             "[BRIDGE main heartbeat] state=%s in_state_ms=%llu current_cmd=%s "
                             "dispatch_msg=0x%X dispatch_hwnd=0x%llx dispatch_in_ms=%llu "
                             "reader_state=%s reader_in_state_ms=%llu queue_size=<locked> "
                             "cmds_received=%llu cmds_processed=%llu\n",
                             bridge_main_state_name(main_state),
                             static_cast<unsigned long long>(now - main_entered),
                             current_cmd.c_str(),
                             dispatch_msg,
                             static_cast<unsigned long long>(dispatch_hwnd),
                             static_cast<unsigned long long>(dispatch_in_ms),
                             bridge_reader_state_name(reader_state),
                             static_cast<unsigned long long>(now - reader_entered),
                             static_cast<unsigned long long>(
                                 reader_cmds_received_.load(std::memory_order_relaxed)),
                             static_cast<unsigned long long>(
                                 main_cmds_processed_.load(std::memory_order_relaxed)));
            }
            std::fflush(stderr);
        }
    }

    // ── メインループ: メッセージポンプ + コマンドキュー処理 ──
    void run_gui_loop() {
        enter_main_state(BridgeMainState::Idle);
        while (running_) {
            enter_main_state(BridgeMainState::PumpingMessages);
            // 1) Win32 メッセージを全消化
            MSG msg;
            while (PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) {
                ::TranslateMessage(&msg);
                const ULONGLONG dispatch_started = GetTickCount64();
                main_dispatch_msg_.store(msg.message, std::memory_order_release);
                main_dispatch_hwnd_.store(
                    reinterpret_cast<uintptr_t>(msg.hwnd),
                    std::memory_order_release);
                main_dispatch_started_tick_.store(dispatch_started, std::memory_order_release);
                ::DispatchMessageW(&msg);
                main_dispatch_started_tick_.store(0, std::memory_order_release);
                main_dispatch_hwnd_.store(0, std::memory_order_release);
                main_dispatch_msg_.store(0, std::memory_order_release);
                const ULONGLONG dispatch_elapsed = GetTickCount64() - dispatch_started;
                if (dispatch_elapsed >= 100 && msg.message != WM_NCLBUTTONDOWN) {
                    std::fprintf(stderr,
                                 "[BRIDGE] slow DispatchMessageW msg=0x%X hwnd=0x%llx elapsed_ms=%llu\n",
                                 msg.message,
                                 static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(msg.hwnd)),
                                 static_cast<unsigned long long>(dispatch_elapsed));
                    std::fflush(stderr);
                }
            }
            // 2) latency_changed 検出 (= プラグインが kLatencyChanged を発火していないか)
            // VST3 では UI でモード切替等が起きると plugin が IComponentHandler::
            // restartComponent(kLatencyChanged) を呼ぶ。ComponentHandler が flag を立て、
            // ここで polling して親プロセス (mIV) に通知する。
            enter_main_state(BridgeMainState::PollingLatency);
            size_t slot_id = 0;
            for (PluginLoader* loader : all_loaders()) {
                uint32_t new_latency = 0;
                if (loader->poll_latency_change(new_latency)) {
                    std::string reply = "{\"event\":\"latency_changed\",\"latency_samples\":" +
                                        std::to_string(new_latency) +
                                        ",\"slot_id\":" + std::to_string(slot_id) + "}";
                    write_message(reply);
                }
                ++slot_id;
            }
            // 3) コマンドキューを 1 件処理
            enter_main_state(BridgeMainState::WaitingCmdQueue);
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
                if (extract_string_field(cmd_msg, "cmd") == "notify_host_resize") {
                    while (!cmd_queue_.empty() &&
                           extract_string_field(cmd_queue_.front(), "cmd") == "notify_host_resize") {
                        cmd_msg = std::move(cmd_queue_.front());
                        cmd_queue_.pop_front();
                    }
                }
            }
            set_current_cmd(extract_string_field(cmd_msg, "cmd"));
            enter_main_state(BridgeMainState::ProcessingCmd);
            const bool keep_running = handle_message(cmd_msg);
            main_cmds_processed_.fetch_add(1, std::memory_order_relaxed);
            set_current_cmd({});
            if (!keep_running) {
                running_ = false;
            }
            enter_main_state(BridgeMainState::Idle);
        }
        enter_main_state(BridgeMainState::Stopped);
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

    static int64_t extract_i64_field(const std::string& json, const std::string& key) {
        std::string needle = "\"" + key + "\"";
        auto pos = json.find(needle);
        if (pos == std::string::npos) return 0;
        pos = json.find(':', pos);
        if (pos == std::string::npos) return 0;
        ++pos;
        while (pos < json.size() && (json[pos] == ' ' || json[pos] == '\t')) ++pos;
        bool neg = false;
        if (pos < json.size() && json[pos] == '-') {
            neg = true;
            ++pos;
        }
        int64_t v = 0;
        while (pos < json.size() && json[pos] >= '0' && json[pos] <= '9') {
            v = v * 10 + (json[pos] - '0');
            ++pos;
        }
        return neg ? -v : v;
    }

    static std::vector<uint64_t> parse_u64_list(const std::string& text) {
        std::vector<uint64_t> out;
        uint64_t value = 0;
        bool have_digit = false;
        for (char ch : text) {
            if (ch >= '0' && ch <= '9') {
                value = value * 10 + static_cast<uint64_t>(ch - '0');
                have_digit = true;
            } else if (ch == ',') {
                if (have_digit) {
                    out.push_back(value);
                }
                value = 0;
                have_digit = false;
            }
        }
        if (have_digit) {
            out.push_back(value);
        }
        return out;
    }

    PluginLoader* loader_at(uint64_t slot_id) {
        std::lock_guard<std::mutex> lk(loaders_mutex_);
        return loader_at_unlocked(slot_id);
    }

    PluginLoader* loader_at_unlocked(uint64_t slot_id) {
        if (slot_id == 0) {
            return loader_.get();
        }
        size_t idx = static_cast<size_t>(slot_id - 1);
        if (idx >= extra_loaders_.size()) {
            return nullptr;
        }
        return extra_loaders_[idx].get();
    }

    PluginLoader* loader_for_message(const std::string& msg) {
        return loader_at(extract_number_field(msg, "slot_id"));
    }

    std::vector<PluginLoader*> all_loaders() {
        std::lock_guard<std::mutex> lk(loaders_mutex_);
        return all_loaders_unlocked();
    }

    std::vector<PluginLoader*> all_loaders_unlocked() {
        std::vector<PluginLoader*> out;
        if (loader_) {
            out.push_back(loader_.get());
        }
        for (auto& loader : extra_loaders_) {
            if (loader) {
                out.push_back(loader.get());
            }
        }
        return out;
    }

    bool slot_bypassed(size_t slot) const {
        return slot < plugin_bypass_.size() && plugin_bypass_[slot];
    }

    void rebuild_chain_snapshot_unlocked() {
        auto next = std::make_shared<ChainSnapshot>();
        next->slot_ids.reserve(processing_order_.size());
        next->loaders.reserve(processing_order_.size());
        next->bypassed.reserve(processing_order_.size());
        for (uint64_t slot_id : processing_order_) {
            PluginLoader* loader = loader_at_unlocked(slot_id);
            if (!loader) continue;
            next->slot_ids.push_back(slot_id);
            next->loaders.push_back(loader);
            next->bypassed.push_back(slot_bypassed(static_cast<size_t>(slot_id)));
        }
        std::atomic_store(&chain_snapshot_, std::shared_ptr<const ChainSnapshot>(next));
    }

    std::shared_ptr<const ChainSnapshot> chain_snapshot() const {
        auto snap = std::atomic_load(&chain_snapshot_);
        if (snap) return snap;
        return std::make_shared<ChainSnapshot>();
    }

    static PluginLoader* loader_from_snapshot(
        const std::shared_ptr<const ChainSnapshot>& snap,
        uint64_t slot_id)
    {
        if (!snap) return nullptr;
        for (size_t i = 0; i < snap->slot_ids.size(); ++i) {
            if (snap->slot_ids[i] == slot_id) {
                return snap->loaders[i];
            }
        }
        return nullptr;
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
        if (cmd == "add_plugin") {
            return handle_add_plugin(msg);
        }
        if (cmd == "probe") {
            std::string plugin_path = extract_string_field(msg, "plugin_path");
            if (plugin_path.empty()) {
                send_event_error("probe: plugin_path missing");
                return true;
            }
            PluginProbeInfo info;
            std::string err;
            if (!PluginLoader::probe(plugin_path, info, err)) {
                send_event_error("probe: " + err);
                return true;
            }
            std::string reply = "{\"event\":\"probed\",\"plugin_name\":\"" +
                                json_escape(info.plugin_name) +
                                "\",\"audio_input_buses\":" +
                                std::to_string(info.audio_input_buses) +
                                ",\"audio_output_buses\":" +
                                std::to_string(info.audio_output_buses) +
                                ",\"event_input_buses\":" +
                                std::to_string(info.event_input_buses) +
                                ",\"event_output_buses\":" +
                                std::to_string(info.event_output_buses) +
                                ",\"audio_input_channels\":" +
                                std::to_string(info.audio_input_channels) +
                                ",\"audio_output_channels\":" +
                                std::to_string(info.audio_output_channels) +
                                ",\"usable_audio_effect\":" +
                                (info.usable_audio_effect ? "true" : "false") +
                                "}";
            write_message(reply);
            return true;
        }
        if (cmd == "reset") {
            // JSON から reset_id を取得 (= 旧 protocol で reset_id 無しの場合は 0)。
            // pending_reset_id_ に格納してから reset_pending_ を立てる順序で memory ordering
            // を担保 (= audio thread は reset_pending_ を見たら pending_reset_id_ も読める)。
            auto id = extract_number_field(msg, "reset_id");
            pending_reset_id_.store(static_cast<uint64_t>(id), std::memory_order_release);
            reset_pending_.store(true, std::memory_order_release);
            return true;
        }
        if (cmd == "query_gui_size") {
            // プラグインの推奨 GUI サイズ + canResize 属性を scale 込みで取得して返す。
            // bridge プロセスは Per-Monitor v2 Aware なので GetDpiForSystem は
            // primary monitor の DPI (= ユーザー環境では 144 等) を返す。
            PluginLoader* loader = loader_for_message(msg);
            if (!loader) {
                send_event_error("query_gui_size: no plugin loaded");
                return true;
            }
            UINT dpi = GetDpiForSystem();
            if (dpi == 0) dpi = 96;
            uint32_t w = 0, h = 0;
            bool resizable = false;
            if (!loader->query_gui_size_at_dpi(dpi, w, h, resizable)) {
                send_event_error("query_gui_size: getSize failed");
                return true;
            }
            std::string reply = "{\"event\":\"gui_size\",\"width\":" +
                                std::to_string(w) + ",\"height\":" +
                                std::to_string(h) +
                                ",\"resizable\":" + (resizable ? "true" : "false") +
                                "}";
            write_message(reply);
            return true;
        }
        if (cmd == "show_gui") {
            uint64_t slot_id = extract_number_field(msg, "slot_id");
            PluginLoader* loader = loader_for_message(msg);
            if (!loader) {
                send_event_error("show_gui: no plugin loaded");
                return true;
            }
            GuiWindowOptions options;
            options.slot_id = slot_id;
            options.owner_hwnd = reinterpret_cast<void*>(extract_number_field(msg, "owner_hwnd"));
            if (!options.owner_hwnd) {
                send_event_error("show_gui: owner_hwnd missing");
                return true;
            }
            options.width = static_cast<uint32_t>(extract_number_field(msg, "width"));
            options.height = static_cast<uint32_t>(extract_number_field(msg, "height"));
            options.resizable = extract_number_field(msg, "resizable") != 0;
            options.has_initial_pos = extract_number_field(msg, "has_initial_pos") != 0;
            options.x = static_cast<int32_t>(extract_i64_field(msg, "x"));
            options.y = static_cast<int32_t>(extract_i64_field(msg, "y"));
            options.title = extract_string_field(msg, "title");
            const bool visible = extract_number_field(msg, "visible") != 0;
            std::string err;
            if (!loader->show_gui(options, visible, err)) {
                send_event_error("show_gui: " + err);
                return true;
            }
            uint32_t w = 0, h = 0;
            const uint64_t container_hwnd =
                reinterpret_cast<uint64_t>(loader->gui_container_hwnd());
            if (loader->get_gui_size(w, h)) {
                std::string reply = "{\"event\":\"gui_attached\",\"width\":" +
                                    std::to_string(w) + ",\"height\":" +
                                    std::to_string(h) +
                                    ",\"slot_id\":" + std::to_string(slot_id) +
                                    ",\"container_hwnd\":" + std::to_string(container_hwnd) + "}";
                write_message(reply);
            } else {
                std::string reply = "{\"event\":\"gui_attached\",\"width\":0,\"height\":0,\"slot_id\":" +
                                    std::to_string(slot_id) +
                                    ",\"container_hwnd\":" + std::to_string(container_hwnd) + "}";
                write_message(reply);
            }
            return true;
        }
        if (cmd == "hide_gui") {
            if (PluginLoader* loader = loader_for_message(msg)) loader->hide_gui();
            write_message("{\"event\":\"gui_detached\"}");
            return true;
        }
        if (cmd == "set_gui_visible") {
            if (PluginLoader* loader = loader_for_message(msg)) {
                uint64_t visible = extract_number_field(msg, "visible");
                uint64_t slot_id = extract_number_field(msg, "slot_id");
                std::fprintf(stderr,
                             "[BRIDGE] set_gui_visible cmd: slot=%llu visible=%llu\n",
                             static_cast<unsigned long long>(slot_id),
                             static_cast<unsigned long long>(visible));
                loader->set_gui_visible(visible != 0);
            }
            return true;
        }
        if (cmd == "set_gui_topmost") {
            if (PluginLoader* loader = loader_for_message(msg)) {
                uint64_t topmost = extract_number_field(msg, "topmost");
                loader->set_gui_topmost(topmost != 0);
            }
            return true;
        }
        if (cmd == "set_gui_app_active") {
            if (PluginLoader* loader = loader_for_message(msg)) {
                uint64_t active = extract_number_field(msg, "active");
                loader->set_gui_app_active(active != 0);
            }
            return true;
        }
        if (cmd == "set_chain_z_order") {
            auto ordered_slots = parse_u64_list(extract_string_field(msg, "ordered_slots"));
            const bool topmost = extract_number_field(msg, "topmost") != 0;
            apply_chain_z_order(ordered_slots, topmost);
            return true;
        }
        if (cmd == "set_chain_visible") {
            auto ordered_slots = parse_u64_list(extract_string_field(msg, "ordered_slots"));
            const bool visible = extract_number_field(msg, "visible") != 0;
            const bool topmost = extract_number_field(msg, "topmost") != 0;
            apply_chain_visible(ordered_slots, visible, topmost);
            return true;
        }
        if (cmd == "set_chain_owner") {
            HWND owner_hwnd = reinterpret_cast<HWND>(extract_number_field(msg, "owner_hwnd"));
            apply_chain_owner(owner_hwnd);
            return true;
        }
        if (cmd == "set_bypass") {
            uint64_t slot = extract_number_field(msg, "slot_id");
            {
                std::lock_guard<std::mutex> lk(loaders_mutex_);
                if (slot < plugin_bypass_.size()) {
                    plugin_bypass_[static_cast<size_t>(slot)] =
                        extract_number_field(msg, "bypass") != 0;
                    rebuild_chain_snapshot_unlocked();
                }
            }
            return true;
        }
        if (cmd == "move_plugin") {
            uint64_t slot = extract_number_field(msg, "slot_id");
            uint64_t before = extract_number_field(msg, "before_slot_id");
            {
                std::lock_guard<std::mutex> lk(loaders_mutex_);
                auto it = std::find(processing_order_.begin(), processing_order_.end(), slot);
                if (it != processing_order_.end()) {
                    processing_order_.erase(it);
                }
                auto before_it = std::find(processing_order_.begin(), processing_order_.end(), before);
                if (before_it != processing_order_.end()) {
                    processing_order_.insert(before_it, slot);
                } else {
                    processing_order_.push_back(slot);
                }
                rebuild_chain_snapshot_unlocked();
            }
            return true;
        }
        if (cmd == "set_passthrough") {
            // 診断用: plugin を経由せず in→out 単純コピー。
            // これで歪みが消えれば plugin process 経路が原因、残れば bridge
            // パイプライン (ring buffer / 変換ロジック) が原因と切り分けられる。
            uint64_t enable = extract_number_field(msg, "enable");
            passthrough_.store(enable != 0, std::memory_order_relaxed);
            return true;
        }
        if (cmd == "notify_host_resize") {
            // host (tester) ウィンドウがユーザーリサイズされた → プラグインに通知して
            // 子ウィンドウを追従させる。応答は不要。
            PluginLoader* loader = loader_for_message(msg);
            if (!loader) return true;
            uint32_t w = static_cast<uint32_t>(extract_number_field(msg, "width"));
            uint32_t h = static_cast<uint32_t>(extract_number_field(msg, "height"));
            if (w > 0 && h > 0) {
                loader->notify_host_resize(w, h);
            }
            return true;
        }
        if (cmd == "set_user_resizing") {
            // ユーザー drag による resize/move session の開始 / 終了通知 (Codex P4)。
            // session 中は plugin の resizeView による host SetWindowPos を抑止して
            // ユーザー drag との衝突 (= ウィンドウ振動) を防ぐ。
            PluginLoader* loader = loader_for_message(msg);
            if (!loader) return true;
            uint64_t active = extract_number_field(msg, "active");
            loader->set_user_resizing(active != 0);
            return true;
        }
        if (cmd == "query_state") {
            // プラグイン内部状態 (= EQ カーブ等) を base64 で取得して送り返す。
            // 親プロセス (mIV) は終了時 / preferences 経由でこれを呼び、
            // settings.json に永続化する → 次回起動時に restore される。
            //
            // **audio thread fence** (Codex P2-2、2026-05-01): control thread からは
            // フラグを立てるだけ。audio thread が loop 境界で getState を実行して
            // event を発行する。process() と並走しないので thread safety を保てる。
            if (!loader_for_message(msg) || !audio_running_) {
                send_event_error("query_state: no plugin loaded");
                return true;
            }
            pending_state_slot_.store(extract_number_field(msg, "slot_id"), std::memory_order_release);
            query_state_pending_.store(true, std::memory_order_release);
            return true;
        }
        if (cmd == "restore_state") {
            // base64 state を decode して audio thread fence 経由で setState する。
            // **初回 auto-restore は Cmd::Open の state field 経由** (= audio_thread 起動前)
            // で適用されるので、こちらは runtime restore (= 将来のプリセット切替等) 用。
            // 現状の使い方では呼ばれない。
            if (!loader_for_message(msg) || !audio_running_) {
                send_event_error("restore_state: no plugin loaded");
                return true;
            }
            std::string b64 = extract_string_field(msg, "state");
            if (b64.empty()) return true;
            std::vector<uint8_t> bytes;
            if (!base64_decode(b64, bytes)) {
                send_event_error("restore_state: invalid base64");
                return true;
            }
            {
                std::lock_guard<std::mutex> lk(restore_state_mutex_);
                restore_state_bytes_ = std::move(bytes);
            }
            pending_state_slot_.store(extract_number_field(msg, "slot_id"), std::memory_order_release);
            restore_state_pending_.store(true, std::memory_order_release);
            return true;
        }
        if (cmd == "close") {
            audio_running_ = false;
            if (audio_thread_.joinable()) audio_thread_.join();
            pipe_.detach();
            {
                std::lock_guard<std::mutex> lk(loaders_mutex_);
                if (loader_) loader_->unload();
                for (auto& loader : extra_loaders_) {
                    if (loader) loader->unload();
                }
                loader_.reset();
                extra_loaders_.clear();
                plugin_bypass_.clear();
                processing_order_.clear();
                rebuild_chain_snapshot_unlocked();
            }
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

        if (audio_thread_.joinable()) {
            audio_running_ = false;
            audio_thread_.join();
            pipe_.detach();
        }

        std::string err;
        if (!pipe_.attach(shm_name, shm_size, sig_in_name, sig_out_name, err)) {
            send_event_error("attach failed: " + err);
            return true;
        }
        sample_rate_ = sample_rate;
        block_size_ = block_size;

        {
            std::lock_guard<std::mutex> lk(loaders_mutex_);
            if (loader_) loader_->unload();
            for (auto& loader : extra_loaders_) {
                if (loader) loader->unload();
            }
            extra_loaders_.clear();
            plugin_bypass_.clear();
            processing_order_.clear();
            loader_ = std::make_unique<PluginLoader>();
            rebuild_chain_snapshot_unlocked();
        }

        LoadedPluginInfo info;
        if (!loader_->load(plugin_path, sample_rate, block_size, info, err)) {
            send_event_error("load failed: " + err);
            {
                std::lock_guard<std::mutex> lk(loaders_mutex_);
                loader_.reset();
                rebuild_chain_snapshot_unlocked();
            }
            pipe_.detach();
            return true;
        }

        // **Initial state apply BEFORE audio_thread starts** (= Codex P2-3、2026-05-01):
        // Cmd::Open に state field を bake することで、audio_thread が動き始める前 (=
        // 完全シングルスレッド) に setState を適用できる。fire-and-forget Cmd::RestoreState
        // 方式と違い「pre-warm が古い state で走る」race が発生しない。
        // 旧 Cmd::RestoreState は runtime 用に残し、audio thread fence 経由で適用する。
        std::string init_state = extract_string_field(msg, "state");
        if (!init_state.empty()) {
            std::vector<uint8_t> bytes;
            if (base64_decode(init_state, bytes)) {
                if (!loader_->restore_state(bytes)) {
                    std::fprintf(stderr,
                        "[BRIDGE] initial restore_state failed (continuing with default)\n");
                }
            } else {
                std::fprintf(stderr,
                    "[BRIDGE] initial state base64 decode failed (continuing with default)\n");
            }
            std::fflush(stderr);
        }
        debug_dump_current_thread_windows("after open_chain load");
        {
            std::lock_guard<std::mutex> lk(loaders_mutex_);
            plugin_bypass_.push_back(false);
            processing_order_.push_back(0);
            rebuild_chain_snapshot_unlocked();
        }

        std::string reply = "{\"event\":\"loaded\",\"plugin_name\":\"" +
                            json_escape(info.plugin_name) +
                            "\",\"slot_id\":0,\"latency_samples\":" +
                            std::to_string(info.latency_samples) + "}";
        write_message(reply);

        audio_running_ = true;
        audio_thread_ = std::thread(&Bridge::audio_loop, this, block_size);
        return true;
    }

    bool handle_add_plugin(const std::string& msg) {
        std::string plugin_path = extract_string_field(msg, "plugin_path");
        if (plugin_path.empty()) {
            send_event_error("add_plugin: plugin_path missing");
            return true;
        }
        if (!audio_running_) {
            send_event_error("add_plugin: chain is not open");
            return true;
        }
        std::fprintf(stderr, "[BRIDGE] add_plugin start path=\"%s\"\n", plugin_path.c_str());
        std::fflush(stderr);
        uint64_t requested_slot = extract_number_field(msg, "slot_id");
        size_t slot_id = 0;
        {
            std::lock_guard<std::mutex> lk(loaders_mutex_);
            slot_id = requested_slot == 0
                ? (extra_loaders_.size() + 1)
                : static_cast<size_t>(requested_slot);
            if (slot_id == 0 || slot_id != extra_loaders_.size() + 1) {
                send_event_error("add_plugin: slot_id must append to the current chain");
                return true;
            }
        }

        auto loader = std::make_unique<PluginLoader>();
        LoadedPluginInfo info;
        std::string err;
        if (!loader->load(plugin_path, sample_rate_, block_size_, info, err)) {
            send_event_error("add_plugin: load failed: " + err);
            return true;
        }
        std::fprintf(stderr,
                     "[BRIDGE] add_plugin load ok slot=%zu name=\"%s\"\n",
                     slot_id,
                     info.plugin_name.c_str());
        std::fflush(stderr);

        std::string init_state = extract_string_field(msg, "state");
        if (!init_state.empty()) {
            std::vector<uint8_t> bytes;
            if (base64_decode(init_state, bytes)) {
                if (!loader->restore_state(bytes)) {
                    std::fprintf(stderr,
                        "[BRIDGE] add_plugin restore_state failed (continuing with default)\n");
                }
            } else {
                std::fprintf(stderr,
                    "[BRIDGE] add_plugin state base64 decode failed (continuing with default)\n");
            }
            std::fflush(stderr);
        }
        debug_dump_current_thread_windows("after add_plugin load");

        {
            std::lock_guard<std::mutex> lk(loaders_mutex_);
            extra_loaders_.push_back(std::move(loader));
            plugin_bypass_.push_back(extract_number_field(msg, "bypass") != 0);
            processing_order_.push_back(slot_id);
            rebuild_chain_snapshot_unlocked();
        }
        std::string reply = "{\"event\":\"loaded\",\"plugin_name\":\"" +
                            json_escape(info.plugin_name) +
                            "\",\"slot_id\":" + std::to_string(slot_id) +
                            ",\"latency_samples\":" +
                            std::to_string(info.latency_samples) + "}";
        write_message(reply);
        return true;
    }

    void apply_chain_owner(HWND owner_hwnd) {
        if (!owner_hwnd || !IsWindow(owner_hwnd)) {
            return;
        }
        std::vector<PluginLoader*> loaders;
        {
            std::lock_guard<std::mutex> lk(loaders_mutex_);
            loaders.reserve(processing_order_.size());
            for (uint64_t slot_id : processing_order_) {
                if (PluginLoader* loader = loader_at_unlocked(slot_id)) {
                    if (loader->is_editor_quarantined()) continue;
                    loaders.push_back(loader);
                }
            }
        }
        for (PluginLoader* loader : loaders) {
            loader->set_gui_owner(owner_hwnd);
        }
    }

    void apply_chain_z_order(const std::vector<uint64_t>& ordered_slots_top_to_bottom,
                             bool topmost) {
        std::vector<HWND> hwnds;
        {
            std::lock_guard<std::mutex> lk(loaders_mutex_);
            hwnds.reserve(ordered_slots_top_to_bottom.size());
            for (uint64_t slot_id : ordered_slots_top_to_bottom) {
                PluginLoader* loader = loader_at_unlocked(slot_id);
                if (!loader) continue;
                if (loader->is_editor_quarantined()) continue;
                HWND hwnd = reinterpret_cast<HWND>(loader->gui_container_hwnd());
                if (hwnd && IsWindow(hwnd)) {
                    hwnds.push_back(hwnd);
                }
            }
        }
        if (hwnds.empty()) return;

        HDWP batch = BeginDeferWindowPos(static_cast<int>(hwnds.size()));
        if (!batch) {
            for (auto it = hwnds.rbegin(); it != hwnds.rend(); ++it) {
                SetWindowPos(*it,
                             topmost ? HWND_TOPMOST : HWND_NOTOPMOST,
                             0, 0, 0, 0,
                             SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
            }
            return;
        }
        HWND insert_after = topmost ? HWND_TOPMOST : HWND_NOTOPMOST;
        for (auto it = hwnds.rbegin(); it != hwnds.rend(); ++it) {
            HDWP next = DeferWindowPos(batch,
                                       *it,
                                       insert_after,
                                       0, 0, 0, 0,
                                       SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
            if (!next) {
                EndDeferWindowPos(batch);
                for (auto fallback = hwnds.rbegin(); fallback != hwnds.rend(); ++fallback) {
                    SetWindowPos(*fallback,
                                 insert_after,
                                 0, 0, 0, 0,
                                 SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
                }
                return;
            }
            batch = next;
        }
        EndDeferWindowPos(batch);
    }

    struct ChainVisibleEntry {
        PluginLoader* loader = nullptr;
        HWND hwnd = nullptr;
        bool show = false;
        bool has_rect = false;
        int32_t x = 0;
        int32_t y = 0;
        int32_t width = 1;
        int32_t height = 1;
    };

    void apply_chain_visible(const std::vector<uint64_t>& ordered_slots_top_to_bottom,
                             bool visible,
                             bool topmost) {
        std::vector<ChainVisibleEntry> entries;
        {
            std::lock_guard<std::mutex> lk(loaders_mutex_);
            entries.reserve(ordered_slots_top_to_bottom.size());
            for (uint64_t slot_id : ordered_slots_top_to_bottom) {
                PluginLoader* loader = loader_at_unlocked(slot_id);
                if (!loader) continue;
                if (loader->is_editor_quarantined()) continue;
                HWND hwnd = reinterpret_cast<HWND>(loader->gui_container_hwnd());
                if (!hwnd || !IsWindow(hwnd)) continue;

                loader->set_gui_surface_visible_state(visible);

                ChainVisibleEntry entry{};
                entry.loader = loader;
                entry.hwnd = hwnd;
                entry.show = loader->gui_surface_should_show();
                if (entry.show) {
                    entry.has_rect = loader->gui_surface_target_rect(entry.x,
                                                                     entry.y,
                                                                     entry.width,
                                                                     entry.height);
                }
                entries.push_back(entry);
            }
        }
        if (entries.empty()) return;

        HDWP batch = BeginDeferWindowPos(static_cast<int>(entries.size()));
        if (!batch) {
            for (auto it = entries.rbegin(); it != entries.rend(); ++it) {
                apply_visible_entry_fallback(*it, topmost);
            }
            refresh_visible_entries(entries);
            return;
        }

        HWND insert_after = topmost ? HWND_TOPMOST : HWND_NOTOPMOST;
        for (auto it = entries.rbegin(); it != entries.rend(); ++it) {
            UINT flags = SWP_NOACTIVATE | SWP_NOOWNERZORDER;
            int x = 0;
            int y = 0;
            int width = 0;
            int height = 0;
            HWND after = insert_after;
            if (it->show) {
                flags |= SWP_SHOWWINDOW;
                if (it->has_rect) {
                    x = it->x;
                    y = it->y;
                    width = it->width;
                    height = it->height;
                } else {
                    flags |= SWP_NOMOVE | SWP_NOSIZE;
                }
            } else {
                flags |= SWP_HIDEWINDOW | SWP_NOMOVE | SWP_NOSIZE;
                after = HWND_NOTOPMOST;
            }

            HDWP next = DeferWindowPos(batch,
                                       it->hwnd,
                                       after,
                                       x,
                                       y,
                                       width,
                                       height,
                                       flags);
            if (!next) {
                EndDeferWindowPos(batch);
                for (auto fallback = entries.rbegin(); fallback != entries.rend(); ++fallback) {
                    apply_visible_entry_fallback(*fallback, topmost);
                }
                refresh_visible_entries(entries);
                return;
            }
            batch = next;
        }
        EndDeferWindowPos(batch);
        refresh_visible_entries(entries);
    }

    static void apply_visible_entry_fallback(const ChainVisibleEntry& entry, bool topmost) {
        if (entry.show) {
            UINT flags = SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW;
            int x = 0;
            int y = 0;
            int width = 0;
            int height = 0;
            if (entry.has_rect) {
                x = entry.x;
                y = entry.y;
                width = entry.width;
                height = entry.height;
            } else {
                flags |= SWP_NOMOVE | SWP_NOSIZE;
            }
            SetWindowPos(entry.hwnd,
                         topmost ? HWND_TOPMOST : HWND_NOTOPMOST,
                         x,
                         y,
                         width,
                         height,
                         flags);
        } else {
            SetWindowPos(entry.hwnd,
                         HWND_NOTOPMOST,
                         0,
                         0,
                         0,
                         0,
                         SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_HIDEWINDOW);
        }
    }

    static void refresh_visible_entries(const std::vector<ChainVisibleEntry>& entries) {
        for (const ChainVisibleEntry& entry : entries) {
            if (entry.show && entry.loader) {
                entry.loader->refresh_gui_surface_now();
            }
        }
    }

    void audio_loop(uint32_t max_block_size) {
        // 可変ブロックサイズモード: tester (cpal) から push されたサンプル数を
        // そのまま 1 ブロックとして処理する (上限は max_block_size = setupProcessing
        // で渡した maxSamplesPerBlock)。
        // 固定 480 で待つと cpal の 441 frame と一致せず周期的アンダーラン
        // (= プチプチノイズ) が発生する問題への対策。
        const uint32_t channels = 2;
        std::vector<float> input(max_block_size * channels);
        std::vector<float> output(max_block_size * channels);
        std::vector<float> temp(max_block_size * channels);

        // ── audio thread を realtime 優先度に上げる ──
        // VST3 host の責務: audio スレッドを GUI thread 等より高優先度にすることで、
        // プラグイン GUI のアナライザ FFT などに割り込まれず一定周期で処理できる。
        // これがないと thread スケジューラ jitter で audio 出力にノイズが乗る。
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);

        // MMCSS (Multimedia Class Scheduler Service) に "Pro Audio" タスクとして
        // 登録すると、Windows audio scheduler から特別待遇を受ける (= プリエンプト
        // されにくい)。WASAPI Exclusive 系の audio app と同等の品質を確保。
        DWORD mmcss_index = 0;
        HANDLE mmcss_handle = AvSetMmThreadCharacteristicsW(L"Pro Audio", &mmcss_index);
        if (mmcss_handle) {
            AvSetMmThreadPriority(mmcss_handle, AVRT_PRIORITY_HIGH);
        }

        // ── Denormal flush を有効化 ──
        // プラグイン内部のフィルタ計算で 1e-30 オーダーの極小値が出ると CPU 計算が
        // 極端に遅くなる (denormal handling)。FTZ/DAZ をセットして「極小値は 0 に
        // 丸める」モードにすることで、フィルタが安定して定時間内に処理される。
        // これがないと CPU spike → 周期的に処理が間に合わない → ノイズ。
        _MM_SET_FLUSH_ZERO_MODE(_MM_FLUSH_ZERO_ON);
        _MM_SET_DENORMALS_ZERO_MODE(_MM_DENORMALS_ZERO_ON);

        std::fprintf(stderr, "[BRIDGE] audio_loop start (max_block=%u, variable size, mmcss=%s)\n",
                     max_block_size, mmcss_handle ? "ok" : "failed");
        std::fflush(stderr);
        uint64_t blocks_in = 0, blocks_processed = 0, blocks_out = 0;
        uint64_t timeouts_in = 0, timeouts_out = 0;
        uint64_t total_frames = 0;
        float input_peak = 0.0f, output_peak = 0.0f;
        uint32_t last_block_frames = 0;
        auto last_report = std::chrono::steady_clock::now();
        auto report_now = [&]() {
            auto now = std::chrono::steady_clock::now();
            if (std::chrono::duration_cast<std::chrono::milliseconds>(now - last_report).count() < 1000) {
                return;
            }
            std::fprintf(stderr,
                "[BRIDGE] audio: in=%llu proc=%llu out=%llu frames=%llu last_blk=%u to_in=%llu to_out=%llu in_peak=%.4f out_peak=%.4f\n",
                (unsigned long long)blocks_in,
                (unsigned long long)blocks_processed,
                (unsigned long long)blocks_out,
                (unsigned long long)total_frames,
                last_block_frames,
                (unsigned long long)timeouts_in,
                (unsigned long long)timeouts_out,
                input_peak, output_peak);
            std::fflush(stderr);
            blocks_in = blocks_processed = blocks_out = 0;
            timeouts_in = timeouts_out = 0;
            total_frames = 0;
            input_peak = output_peak = 0.0f;
            last_report = now;
        };

        while (audio_running_) {
            // ── reset fence (= Codex 助言、2026-05-01) ──
            // control thread が `reset_pending_` を立てたら、ここで in/out ring を
            // drain して plugin を reset する。process と setProcessing は同一 thread で
            // 直列化されるので race なし。
            if (reset_pending_.exchange(false, std::memory_order_acq_rel)) {
                uint64_t reset_id = pending_reset_id_.load(std::memory_order_acquire);
                pipe_.discard_all();
                uint32_t total_latency = 0;
                uint32_t active_loaders = 0;
                auto snap = chain_snapshot();
                for (size_t i = 0; i < snap->loaders.size(); ++i) {
                    PluginLoader* loader = snap->loaders[i];
                    if (!loader || snap->bypassed[i]) continue;
                    ++active_loaders;
                    loader->reset();
                    uint32_t lat = loader->latency_samples();
                    total_latency += lat;
                    if (lat > 0) {
                        loader->flush_with_silence(lat);
                    }
                }
                if (active_loaders > 0) {
                    std::fprintf(stderr,
                                 "[BRIDGE] reset done id=%llu (in/out ring drained, %u plugins reset, %u samples silence flushed)\n",
                                 (unsigned long long)reset_id,
                                 active_loaders,
                                 total_latency);
                } else {
                    std::fprintf(stderr, "[BRIDGE] reset done id=%llu (no loader)\n",
                                 (unsigned long long)reset_id);
                }
#if 0
                if (loader_) {
                    loader_->reset();
                    // setProcessing(false/true) の後に **silence で delay-line を埋める**。
                    // VST3 仕様上 setProcessing は "should clear internal state" であり、
                    // 全 plugin が delay-line をクリアする保証はないため、明示的に
                    // latency_samples 分の silence を流して plugin output が silence で
                    // 始まることを保証する (= シーク後 pre-seek audio 残留の確実防止)。
                    uint32_t lat = loader_->latency_samples();
                    if (lat > 0) {
                        loader_->flush_with_silence(lat);
                        std::fprintf(stderr, "[BRIDGE] reset done id=%llu (in/out ring drained, plugin reset, %u samples silence flushed)\n",
                                     (unsigned long long)reset_id, lat);
                    } else {
                        std::fprintf(stderr, "[BRIDGE] reset done id=%llu (in/out ring drained, plugin reset, no latency)\n",
                                     (unsigned long long)reset_id);
                    }
                } else {
                    std::fprintf(stderr, "[BRIDGE] reset done id=%llu (no loader)\n",
                                 (unsigned long long)reset_id);
                }
#endif
                std::fflush(stderr);
                // ack に reset_id をエコー (= mIV 側 wait が ID 照合する)
                std::string reply = "{\"event\":\"reset_done\",\"reset_id\":" +
                                    std::to_string(reset_id) + "}";
                write_message(reply);
                // Rust pump は reset_done ack 待ちなので、次 push まで待つ。
                continue;
            }
            uint32_t got = pipe_.read_in_available(input.data(),
                                                    max_block_size * channels,
                                                    100 /* ms */);

            // ── State op fence (Codex P2-2、2026-05-01) ──
            // control thread が立てた query_state / restore_state を audio thread 上で
            // ここで実行する。read の **後**・process の **前** に置くことで:
            // - process と setState/getState が並走しない (= thread safety)
            // - control が flag を立てた直後に push された input は、setState 反映済の
            //   状態で次の process_block に渡る (= pre-warm が古い state で走る race を防止)
            if (query_state_pending_.exchange(false, std::memory_order_acq_rel)) {
                uint64_t slot_id = pending_state_slot_.load(std::memory_order_acquire);
                auto snap = chain_snapshot();
                PluginLoader* loader = loader_from_snapshot(snap, slot_id);
                if (loader) {
                    std::vector<uint8_t> bytes;
                    if (loader->query_state(bytes)) {
                        std::string b64 = base64_encode(bytes);
                        std::string reply = "{\"event\":\"plugin_state\",\"state\":\"" +
                                            b64 + "\",\"slot_id\":" +
                                            std::to_string(slot_id) + "}";
                        write_message(reply);
                    } else {
                        send_event_error("query_state: getState failed");
                    }
                }
            }
            if (restore_state_pending_.exchange(false, std::memory_order_acq_rel)) {
                std::vector<uint8_t> bytes;
                {
                    std::lock_guard<std::mutex> lk(restore_state_mutex_);
                    bytes = std::move(restore_state_bytes_);
                }
                uint64_t slot_id = pending_state_slot_.load(std::memory_order_acquire);
                auto snap = chain_snapshot();
                PluginLoader* loader = loader_from_snapshot(snap, slot_id);
                if (loader && !bytes.empty()) {
                    if (!loader->restore_state(bytes)) {
                        send_event_error("restore_state: setState failed");
                    }
                }
            }

            if (got == 0) {
                ++timeouts_in;
                report_now();
                if (!audio_running_) break;
                continue;
            }
            // 必ず channels の倍数に揃える (= 半端な 1 sample があれば次回に持ち越す)。
            uint32_t aligned = got - (got % channels);
            if (aligned == 0) {
                continue;
            }
            // ※ もし got != aligned なら半端 sample を捨てている形。read_in_available の
            // 設計上、push 側 (tester) も channel-aligned で push しているはずなので
            // 通常は got % channels == 0 になる。
            uint32_t frames = aligned / channels;
            ++blocks_in;
            last_block_frames = frames;
            total_frames += frames;
            for (uint32_t i = 0; i < aligned; ++i) {
                input_peak = std::max(input_peak, std::fabs(input[i]));
            }

            if (passthrough_.load(std::memory_order_relaxed)) {
                // 診断用パススルー: plugin 経由せずそのままコピー
                std::memcpy(output.data(), input.data(), aligned * sizeof(float));
            } else {
                bool processed_any = false;
                const float* current_in = input.data();
                float* current_out = output.data();
                auto snap = chain_snapshot();
                for (size_t i = 0; i < snap->loaders.size(); ++i) {
                    PluginLoader* loader = snap->loaders[i];
                    if (!loader || snap->bypassed[i]) continue;
                    current_out = processed_any
                        ? (current_out == output.data() ? temp.data() : output.data())
                        : output.data();
                    if (!loader->process_block(current_in, current_out, frames)) {
                        send_event_error("process_block failed");
                        audio_running_ = false;
                        break;
                    }
                    processed_any = true;
                    current_in = current_out;
                }
                if (!audio_running_) {
                    break;
                }
                if (!processed_any) {
                    std::memcpy(output.data(), input.data(), aligned * sizeof(float));
                } else if (current_in != output.data()) {
                    std::memcpy(output.data(), current_in, aligned * sizeof(float));
                }
            }
            ++blocks_processed;
            for (uint32_t i = 0; i < aligned; ++i) {
                output_peak = std::max(output_peak, std::fabs(output[i]));
            }

            if (!pipe_.write_out(output.data(), aligned, 100 /* ms */)) {
                ++timeouts_out;
                report_now();
                if (!audio_running_) break;
                continue;
            }
            ++blocks_out;
            report_now();
        }
        if (mmcss_handle) {
            AvRevertMmThreadCharacteristics(mmcss_handle);
        }
        std::fprintf(stderr, "[BRIDGE] audio_loop exit\n");
        std::fflush(stderr);
    }

    std::mutex loaders_mutex_;
    std::unique_ptr<PluginLoader> loader_;
    std::vector<std::unique_ptr<PluginLoader>> extra_loaders_;
    std::vector<bool> plugin_bypass_;
    std::vector<uint64_t> processing_order_;
    std::shared_ptr<const ChainSnapshot> chain_snapshot_;
    AudioPipe pipe_;
    uint32_t sample_rate_ = 0;
    uint32_t block_size_ = 0;

    // GUI / コマンドキュー
    std::mutex cmd_mutex_;
    std::condition_variable cmd_cv_;
    std::deque<std::string> cmd_queue_;
    std::atomic<bool> running_{false};

    // watchdog diagnostics for bridge main/reader stalls
    std::atomic<int> main_state_{static_cast<int>(BridgeMainState::Starting)};
    std::atomic<int> reader_state_{static_cast<int>(BridgeReaderState::Starting)};
    std::atomic<ULONGLONG> main_state_entered_tick_{0};
    std::atomic<ULONGLONG> reader_state_entered_tick_{0};
    std::atomic<uint64_t> reader_cmds_received_{0};
    std::atomic<uint64_t> main_cmds_processed_{0};
    std::atomic<UINT> main_dispatch_msg_{0};
    std::atomic<uintptr_t> main_dispatch_hwnd_{0};
    std::atomic<ULONGLONG> main_dispatch_started_tick_{0};
    std::mutex state_mutex_;
    std::string current_cmd_;

    // audio
    std::thread audio_thread_;
    std::atomic<bool> audio_running_{false};
    // 診断用 passthrough flag (= true なら plugin を経由しない)
    std::atomic<bool> passthrough_{false};
    // シーク時 reset 用 fence。control thread (= run_gui_loop で `cmd == "reset"`) が
    // この atomic を立て、audio thread が loop 先頭で exchange して true なら
    // in/out ring を drain + plugin reset + reset_done event を出す。
    // これにより GUI/control thread と audio thread の race を排除し、
    // process と setProcessing(false/true) を audio thread 上で直列化する
    // (= Codex 助言、2026-05-01、VST3 公式 threading model にも整合)。
    std::atomic<bool> reset_pending_{false};
    /// `Cmd::Reset { reset_id }` の reset_id を保持する。control thread が
    /// `reset_pending_` を立てる際にここに ID を書き込み、audio thread が reset 実行後
    /// `Event::ResetDone { reset_id }` で返す。これにより mIV 側は stale ack race を
    /// 防げる (= Codex 助言、2026-05-01)。
    /// reset は同期呼び出し前提のため、複数 reset が短時間連続する場合は最後の ID で
    /// coalesce される (= bool + latest id 設計、堅牢化が必要なら queue 化)。
    std::atomic<uint64_t> pending_reset_id_{0};
    /// runtime state op (= query_state / restore_state) を audio thread 上で実行する
    /// ための fence (Codex P2-2、2026-05-01)。control thread はフラグを立てるだけ、
    /// audio thread が loop 境界で flag を exchange して loader_ を排他に触る。
    /// VST3 plugin の thread safety を担保し、process と setState/getState の race を排除。
    std::atomic<bool> query_state_pending_{false};
    std::atomic<bool> restore_state_pending_{false};
    std::atomic<uint64_t> pending_state_slot_{0};
    std::mutex restore_state_mutex_;          // protects restore_state_bytes_
    std::vector<uint8_t> restore_state_bytes_;
};

}  // namespace miv

static DWORD parse_parent_pid_arg(int argc, char** argv) {
    for (int i = 1; i + 1 < argc; ++i) {
        if (std::strcmp(argv[i], "--parent-pid") == 0) {
            char* end = nullptr;
            unsigned long value = std::strtoul(argv[i + 1], &end, 10);
            if (end && *end == '\0' && value != 0) {
                return static_cast<DWORD>(value);
            }
        }
    }
    return 0;
}

static void start_parent_watchdog(DWORD parent_pid) {
    if (parent_pid == 0) {
        return;
    }
    HANDLE parent = OpenProcess(SYNCHRONIZE, FALSE, parent_pid);
    if (!parent) {
        std::fprintf(stderr,
                     "[BRIDGE] parent watchdog: OpenProcess failed pid=%lu err=%lu\n",
                     static_cast<unsigned long>(parent_pid),
                     GetLastError());
        std::fflush(stderr);
        return;
    }
    std::thread([parent, parent_pid]() {
        DWORD wait_result = WaitForSingleObject(parent, INFINITE);
        std::fprintf(stderr,
                     "[BRIDGE] parent watchdog: parent exited pid=%lu wait=%lu, exiting bridge\n",
                     static_cast<unsigned long>(parent_pid),
                     wait_result);
        std::fflush(stderr);
        CloseHandle(parent);
        ExitProcess(0);
    }).detach();
}

int main(int argc, char** argv) {
    // bridge プロセスを Per-Monitor v2 DPI Aware に設定する。
    // これがないと GetDpiForSystem / GetDpiForWindow がプライマリ DPI ではなく
    // 96 を返してしまい、setContentScaleFactor で正しい scale を伝えられない。
    // VST3 GUI を任意のスレッドで attached する前に必ずプロセス全体に設定する必要がある。
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    start_parent_watchdog(parse_parent_pid_arg(argc, argv));
    miv::Bridge bridge;
    return bridge.run();
}
