// Shared memory + named events で親プロセスと音声を交換する SPSC リング実装。

#include "audio_pipe.h"

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

    // ring 配置: header の直後に in_ring、その後に out_ring
    auto* base = reinterpret_cast<uint8_t*>(shm_base_);
    in_ring_ = reinterpret_cast<float*>(base + sizeof(ShmHeader));
    out_ring_ = in_ring_ + header_->capacity;

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

    // ring 読み出し
    uint32_t cap = header_->capacity;
    for (uint32_t i = 0; i < num_samples; ++i) {
        out[i] = in_ring_[(r_pos + i) % cap];
    }
    r.store(r_pos + num_samples, std::memory_order_release);
    return true;
}

bool AudioPipe::write_out(const float* in, uint32_t num_samples, uint32_t timeout_ms) {
    if (!header_) return false;
    auto& w = *reinterpret_cast<std::atomic<uint32_t>*>(&header_->out_write);
    auto& r = *reinterpret_cast<std::atomic<uint32_t>*>(&header_->out_read);

    uint32_t cap = header_->capacity;
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

}  // namespace miv
