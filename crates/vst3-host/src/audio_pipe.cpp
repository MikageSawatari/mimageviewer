// Shared memory + named events で親プロセスと音声を交換する SPSC リング実装。

#include "audio_pipe.h"

#include <algorithm>
#include <atomic>
#include <cstring>
#include <string>

namespace miv {

namespace {
// std::string (UTF-8) → wide string. shm 名や event 名はパス区切りなしの ASCII 想定なので
// 単純変換で OK。
std::wstring to_wide(const std::string& s) {
    std::wstring w(s.size(), L'\0');
    for (size_t i = 0; i < s.size(); ++i) {
        w[i] = static_cast<wchar_t>(static_cast<unsigned char>(s[i]));
    }
    return w;
}
}  // namespace

AudioPipe::AudioPipe() = default;

AudioPipe::~AudioPipe() {
    detach();
}

bool AudioPipe::attach(const std::string& shm_name,
                        uint64_t shm_size,
                        const std::string& sig_in_name,
                        const std::string& sig_out_name,
                        std::string& error_out) {
    detach();

    // T08 (v0.9.0): shm_size 自体の sanity check を最初に。header と 2 本の ring が
    // 全部入らなければそもそも何もできない。
    if (shm_size < sizeof(ShmHeader)) {
        error_out = "shm_size too small for ShmHeader";
        return false;
    }

    auto wshm = to_wide(shm_name);
    shm_handle_ = OpenFileMappingW(FILE_MAP_ALL_ACCESS, FALSE, wshm.c_str());
    if (!shm_handle_) {
        error_out = "OpenFileMappingW failed for " + shm_name;
        return false;
    }
    shm_base_ = MapViewOfFile(shm_handle_, FILE_MAP_ALL_ACCESS, 0, 0,
                                static_cast<SIZE_T>(shm_size));
    if (!shm_base_) {
        error_out = "MapViewOfFile failed";
        detach();
        return false;
    }
    shm_size_ = shm_size;
    header_ = reinterpret_cast<ShmHeader*>(shm_base_);

    // T08 (v0.9.0): header の中身を **ポインタ算術より前** に検証する。攻撃者やバグが
    // capacity = 0 や巨大値、channels の不正値を書いていた場合、`out_ring_ = in_ring_ +
    // header_->capacity` が out-of-bounds ポインタを生み、後段の `% header_->capacity` が
    // ゼロ除算クラッシュやマップ範囲外読み書きを起こす。
    //
    // 厳密一致 (= block_size * channels * 8) は将来の ring サイズ変更を縛るので不採用。
    // 「整合的に使える境界」だけを最小条件として課す。
    const uint32_t capacity = header_->capacity;
    const uint32_t channels = header_->channels;
    const uint32_t block_size = header_->block_size;
    constexpr uint32_t kMaxCapacitySamples = 16u * 1024u * 1024u; // 16 MiB samples (= 64 MiB stereo)
    if (capacity == 0 || capacity > kMaxCapacitySamples) {
        error_out = "ShmHeader.capacity out of range (0 or > 16 MiB samples)";
        detach();
        return false;
    }
    // T08 (v0.9.0) Codex P3 反映: bridge audio_loop は現在ハードコードで stereo を
    // 前提に処理する (main.cpp の固定 channel 数経路)。mono サポートは v0.10 以降に
    // 揃ってから許可するため、ここでは厳格に channels == 2 を要求する。`cached_channels_`
    // の値もこの validate を通った後の `channels` を保持する。
    if (channels != 2) {
        error_out = "ShmHeader.channels must be 2 (mono not yet supported)";
        detach();
        return false;
    }
    // channels が 2 固定なので、capacity も偶数でなければ stereo frame の整数倍にならない。
    if (capacity % 2 != 0) {
        error_out = "ShmHeader.capacity must be even (stereo frame boundary)";
        detach();
        return false;
    }
    if (block_size == 0) {
        error_out = "ShmHeader.block_size is zero";
        detach();
        return false;
    }
    // ring 1 つ分の最低サイズ = block_size * channels (= 1 ブロックぶん入る)
    if (capacity < static_cast<uint64_t>(block_size) * channels) {
        error_out = "ShmHeader.capacity smaller than one block";
        detach();
        return false;
    }
    // mapping 全体に header + 2 本の ring が収まるか (u64 で overflow 回避)
    const uint64_t total_bytes =
        static_cast<uint64_t>(sizeof(ShmHeader)) +
        static_cast<uint64_t>(2u) * static_cast<uint64_t>(capacity) * sizeof(float);
    if (total_bytes > shm_size_) {
        error_out = "ShmHeader.capacity overruns the mapped shm region";
        detach();
        return false;
    }
    cached_capacity_ = capacity;
    cached_channels_ = channels;

    // ring 配置: header の直後に in_ring、その後に out_ring
    auto* base = reinterpret_cast<uint8_t*>(shm_base_);
    in_ring_ = reinterpret_cast<float*>(base + sizeof(ShmHeader));
    out_ring_ = in_ring_ + cached_capacity_;

    auto wsi = to_wide(sig_in_name);
    sig_in_ = OpenEventW(EVENT_ALL_ACCESS, FALSE, wsi.c_str());
    if (!sig_in_) {
        error_out = "OpenEventW failed for sig_in";
        detach();
        return false;
    }
    auto wso = to_wide(sig_out_name);
    sig_out_ = OpenEventW(EVENT_ALL_ACCESS, FALSE, wso.c_str());
    if (!sig_out_) {
        error_out = "OpenEventW failed for sig_out";
        detach();
        return false;
    }
    return true;
}

void AudioPipe::detach() {
    if (sig_in_) { CloseHandle(sig_in_); sig_in_ = nullptr; }
    if (sig_out_) { CloseHandle(sig_out_); sig_out_ = nullptr; }
    if (shm_base_) { UnmapViewOfFile(shm_base_); shm_base_ = nullptr; }
    if (shm_handle_) { CloseHandle(shm_handle_); shm_handle_ = nullptr; }
    shm_size_ = 0;
    header_ = nullptr;
    in_ring_ = nullptr;
    out_ring_ = nullptr;
    cached_capacity_ = 0;
    cached_channels_ = 0;
}

bool AudioPipe::read_in(float* out, uint32_t num_samples, uint32_t timeout_ms) {
    if (!header_) return false;
    auto& w = *reinterpret_cast<std::atomic<uint32_t>*>(&header_->in_write);
    auto& r = *reinterpret_cast<std::atomic<uint32_t>*>(&header_->in_read);

    uint32_t r_pos = r.load(std::memory_order_relaxed);
    uint32_t w_pos = w.load(std::memory_order_acquire);
    uint32_t avail = w_pos - r_pos;  // wrapping unsigned subtraction

    while (avail < num_samples) {
        if (WaitForSingleObject(sig_in_, timeout_ms) != WAIT_OBJECT_0) {
            return false;  // timeout / abandoned
        }
        w_pos = w.load(std::memory_order_acquire);
        avail = w_pos - r_pos;
    }

    // ring 読み出し (T08: validate 済み cached_capacity_ を使う)
    uint32_t cap = cached_capacity_;
    for (uint32_t i = 0; i < num_samples; ++i) {
        out[i] = in_ring_[(r_pos + i) % cap];
    }
    r.store(r_pos + num_samples, std::memory_order_release);
    return true;
}

uint32_t AudioPipe::read_in_available(float* out, uint32_t max_samples, uint32_t timeout_ms) {
    if (!header_) return 0;
    auto& w = *reinterpret_cast<std::atomic<uint32_t>*>(&header_->in_write);
    auto& r = *reinterpret_cast<std::atomic<uint32_t>*>(&header_->in_read);

    uint32_t r_pos = r.load(std::memory_order_relaxed);
    uint32_t w_pos = w.load(std::memory_order_acquire);
    uint32_t avail = w_pos - r_pos;

    if (avail == 0) {
        if (WaitForSingleObject(sig_in_, timeout_ms) != WAIT_OBJECT_0) {
            return 0;  // timeout
        }
        w_pos = w.load(std::memory_order_acquire);
        avail = w_pos - r_pos;
        if (avail == 0) return 0;
    }

    uint32_t to_read = std::min(avail, max_samples);
    uint32_t cap = cached_capacity_;
    for (uint32_t i = 0; i < to_read; ++i) {
        out[i] = in_ring_[(r_pos + i) % cap];
    }
    r.store(r_pos + to_read, std::memory_order_release);
    return to_read;
}

bool AudioPipe::write_out(const float* in, uint32_t num_samples, uint32_t timeout_ms) {
    if (!header_) return false;
    auto& w = *reinterpret_cast<std::atomic<uint32_t>*>(&header_->out_write);
    auto& r = *reinterpret_cast<std::atomic<uint32_t>*>(&header_->out_read);

    uint32_t cap = cached_capacity_;
    uint32_t w_pos = w.load(std::memory_order_relaxed);
    uint32_t r_pos = r.load(std::memory_order_acquire);
    uint32_t free_space = cap - (w_pos - r_pos);

    // out_ring が満杯 = 親が消費するまで待つ。
    // ただし POC では cpal が常に消費しに来るので、ほぼ即時で空く前提。
    auto deadline = GetTickCount64() + timeout_ms;
    while (free_space < num_samples) {
        uint64_t now = GetTickCount64();
        if (now >= deadline) return false;
        // 親側が「out_ring 読んだぞ」のシグナルを出すのが理想だが、
        // POC では sig_in (= 親が次のブロックを書いた時に発火) を流用して
        // ポーリング状で再チェックする (短時間なので問題なし)。
        Sleep(1);
        r_pos = r.load(std::memory_order_acquire);
        free_space = cap - (w_pos - r_pos);
    }

    for (uint32_t i = 0; i < num_samples; ++i) {
        out_ring_[(w_pos + i) % cap] = in[i];
    }
    w.store(w_pos + num_samples, std::memory_order_release);
    SetEvent(sig_out_);
    return true;
}

void AudioPipe::wake_input() {
    if (sig_in_) {
        SetEvent(sig_in_);
    }
}

void AudioPipe::discard_all() {
    if (!header_) return;
    // SPSC 規則上、in_ring は consumer (= bridge audio thread) のみが read 側 index を
    // 進めてよく、out_ring は producer (= bridge audio thread) のみが write 側 index を
    // 進めてよい。reset fence では Rust 側が push_audio / pull_audio を停止しているので
    // 一時的に producer/consumer 両方の index に触れて全 discard する。
    //
    // 実装: 両 ring とも read 側を write 側まで一気に進める (= 「全部読んだ扱い」)。
    // これで未消費 sample がすべて捨てられる。物理的に zero-fill する必要はない
    // (= 次に書き込まれるとき index ベースで上書きされるだけ)。
    auto& in_w = *reinterpret_cast<std::atomic<uint32_t>*>(&header_->in_write);
    auto& in_r = *reinterpret_cast<std::atomic<uint32_t>*>(&header_->in_read);
    auto& out_w = *reinterpret_cast<std::atomic<uint32_t>*>(&header_->out_write);
    auto& out_r = *reinterpret_cast<std::atomic<uint32_t>*>(&header_->out_read);

    in_r.store(in_w.load(std::memory_order_acquire), std::memory_order_release);
    out_r.store(out_w.load(std::memory_order_acquire), std::memory_order_release);
}

}  // namespace miv
