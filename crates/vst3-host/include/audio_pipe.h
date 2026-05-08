// 親プロセスとの音声受け渡し: shared memory + named events
//
// 親が in_ring に PCM を書く → SetEvent(sig_in) → bridge が WaitForSingleObject(sig_in)
// → bridge が in_ring を読み、プラグインで処理し、out_ring に書く →
// SetEvent(sig_out) → 親が WaitForSingleObject(sig_out) → 親が out_ring から取り出す。
//
// 各 ring は 1 production / 1 consumption スレッドのみが触る (SPSC) ので、
// `std::atomic<uint32_t>` の write/read index で lock-free に同期できる。
// memory ordering は acquire/release で十分。
//
// 容量は block_size の 8 倍程度 (= 8 ブロック分のマージン) に取る。
// プラグイン処理時間が変動しても underrun / overrun を起こしにくくする。

#pragma once

#include <cstdint>
#include <string>
#include <windows.h>

#include "protocol.h"

namespace miv {

class AudioPipe {
public:
    AudioPipe();
    ~AudioPipe();

    // 親からの "ready" メッセージで通知された shared memory にアタッチする。
    // shm_name / sig_in_name / sig_out_name は親が CreateFileMappingW / CreateEventW で
    // 作ったもの。bridge は OpenFileMappingW / OpenEventW で開く。
    // 失敗時は false を返し、error_out にエラーメッセージを入れる。
    bool attach(const std::string& shm_name,
                uint64_t shm_size,
                const std::string& sig_in_name,
                const std::string& sig_out_name,
                std::string& error_out);

    void detach();

    // in_ring から num_frames * channels 個の f32 sample を読む。
    // 読めるサンプル数が足りなければ sig_in を待つ (タイムアウト ms 指定)。
    // タイムアウトすると false を返す。
    bool read_in(float* out, uint32_t num_samples, uint32_t timeout_ms);

    /// in_ring から **来ている分だけ** 読む (最大 max_samples まで)。
    /// 1 sample も来ていなければ sig_in を待ち、待てた分を返す。
    /// 戻り値: 実際に読んだ sample 数 (0 ならタイムアウト)。
    /// cpal の callback サイズと bridge の固定 block_size が一致しないときに、
    /// 「来た分だけ即時処理」する可変ブロックサイズモード用。
    uint32_t read_in_available(float* out, uint32_t max_samples, uint32_t timeout_ms);

    // out_ring に num_samples 個の f32 を書き、sig_out を発火する。
    // ring が満杯なら sig_in (= 親が読み出すまで) を待つ仕様にしてもよいが、
    // 通常 cpal 側が確実に読みに来るので timeout_ms 内で書ければ OK。
    bool write_out(const float* in, uint32_t num_samples, uint32_t timeout_ms);

    /// 入力待ちで眠っている bridge audio thread を起こす。
    /// reset/query/shutdown など、音声サンプルを伴わない制御要求を即時処理するために使う。
    void wake_input();

    /// in_ring と out_ring の両方の未消費データをすべて捨てる。
    /// シーク等で plugin reset を行うときに、pre-seek の audio が残ったまま
    /// 新しい状態に進まないよう、reset fence の一部として呼ぶ。
    /// **呼び出し前提**: Rust 側が `reset_plugins_sync` で ack 待ちに入っており、
    /// 並行して push_audio / pull_audio をしていないこと (= SPSC 規則を一時的に破る)。
    void discard_all();

    const ShmHeader* header() const { return header_; }

private:
    HANDLE shm_handle_ = nullptr;
    HANDLE sig_in_ = nullptr;
    HANDLE sig_out_ = nullptr;
    void* shm_base_ = nullptr;
    uint64_t shm_size_ = 0;

    ShmHeader* header_ = nullptr;
    float* in_ring_ = nullptr;   // header の直後
    float* out_ring_ = nullptr;  // in_ring の直後
};

}  // namespace miv
