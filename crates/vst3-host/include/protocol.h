// mimageviewer-vst3-host IPC プロトコル定義
//
// 親プロセス (Rust) ↔ bridge プロセス (この C++ exe) 間の通信フォーマット。
// stdin/stdout は **制御用** (低頻度メッセージ: ロード命令、latency 通知、エラー)。
// 音声バッファは **shared memory ring buffer** で渡す (高頻度、低レイテンシ)。
//
// ## 制御メッセージ (stdin/stdout)
//
// 各メッセージは「4 byte LE length」+「length バイトの payload (UTF-8 JSON)」。
// stdin がクローズされたら bridge は graceful shutdown する。
//
// ### 親 → bridge (要求):
//   {"cmd":"hello","version":1}
//   {"cmd":"open","plugin_path":"<UTF-8 絶対パス>","sample_rate":48000,"block_size":480}
//   {"cmd":"set_param","id":<u32>,"value":<f64 [0..1]>}
//   {"cmd":"reset"}                 // 再生位置変更 / プラグイン状態リセット
//   {"cmd":"close"}                 // プラグインアンロード
//   {"cmd":"shutdown"}              // bridge 終了
//
// ### bridge → 親 (応答 / 通知):
//   {"event":"ready","version":1,"shm_name":"<Windows 名前付きオブジェクト>",
//    "shm_size":<u64>,"sig_in":"<event名>","sig_out":"<event名>"}
//   {"event":"loaded","plugin_name":"<UTF-8>","latency_samples":<u32>,
//    "params":[{"id":<u32>,"name":"<UTF-8>","default":<f64>}, ...]}
//   {"event":"latency_changed","latency_samples":<u32>}
//   {"event":"error","detail":"<UTF-8>"}
//   {"event":"closed"}
//
// ## Shared memory layout
//
// 1 つの名前付き shared memory に **2 本の SPSC リング** を配置する:
//   - in_ring:  親が書き込み、bridge が読む (= bridge への入力 PCM)
//   - out_ring: bridge が書き込み、親が読む (= bridge からの出力 PCM)
//
// それぞれの ring は cache-line aligned (64 byte) な atomic head/tail と
// f32 サンプルバッファから成る。サンプルは f32 stereo packed (= [L, R, L, R, ...])。
//
// 同期は Windows named event (CreateEventW) 2 本:
//   - sig_in:  親が in_ring に書き込み終わったら SetEvent。bridge が WaitForSingleObject
//   - sig_out: bridge が out_ring に書き込み終わったら SetEvent。親が待ち合わせ
//
// 音声経路に Windows event を挟む理由: cpal RT callback は 10ms 以内で返らないと
// アンダーランするが、event の SetEvent / WaitForSingleObject は数 µs オーダーで
// 完了し、context switch コストも 10〜100 µs 程度。busy-wait は CLAUDE.md ポリシー違反。

#pragma once

#include <cstdint>

namespace miv {

// 共有メモリのレイアウト header。block_size は openコマンドで親が指定する値。
// in_ring / out_ring の容量はそれぞれ block_size * 4 sample (8 ブロック分マージン)。
//
// **重要**: Rust 側 (bridge.rs::ShmHeader) と **完全に同じバイトレイアウト** に
// なっていなければならない。`alignas(64)` を使うと struct 全体が 64 byte 倍数に
// padding されて Rust 側 (`#[repr(C)]` で 272 byte) とサイズがずれる。
// 各 atomic は **手動 padding (uint8_t[60])** で cache-line 分離する。
// これで C++ sizeof = Rust size_of = 272 byte で一致。
struct ShmHeader {
    uint32_t in_write;          // 親が書いた累積 sample 数 (mono = 1, stereo = 2)
    uint8_t _pad0[60];
    uint32_t in_read;           // bridge が読んだ累積
    uint8_t _pad1[60];
    uint32_t out_write;         // bridge が書いた累積
    uint8_t _pad2[60];
    uint32_t out_read;          // 親が読んだ累積
    uint8_t _pad3[60];
    // 容量 (sample 数、stereo なら *2 されている)。両 ring 共通。
    uint32_t capacity;
    uint32_t channels;          // 通常 2 (stereo)
    uint32_t sample_rate;       // 48000 等
    uint32_t block_size;        // 1 ブロックの sample 数
};
static_assert(sizeof(ShmHeader) == 272,
              "ShmHeader must be 272 bytes to match Rust side");

constexpr uint32_t PROTOCOL_VERSION = 1;

// 制御メッセージの最大長。
// 通常は 1 KB 未満だが、`plugin_state` event (= IComponent::getState チャンク) は
// プラグインによっては数百 KB に達する (= ML 系プラグインや、波形 / プリセット
// 内蔵プラグイン)。base64 化で +33% の overhead を考えても、4 MB あれば実用上
// 困らない。Rust 側 `read_event_blocking` も同じ上限。
constexpr uint32_t MAX_CONTROL_MSG_SIZE = 4 * 1024 * 1024;

}  // namespace miv
