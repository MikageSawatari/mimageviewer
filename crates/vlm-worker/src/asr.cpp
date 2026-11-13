#include "asr.h"
#include "asr_protocol.h"
#include "asr_window.h"
#include "devices.h"
#include "build_identity.h"
#include "whisper.h"
#include <atomic>
#include <chrono>
#include <cstdio>
#include <cstring>
#include <exception>
#include <fcntl.h>
#include <io.h>
#include <iostream>
#include <memory>
#include <thread>

namespace {
using json = nlohmann::json;
struct WhisperDeleter { void operator()(whisper_context * context) const { whisper_free(context); } };
using Context = std::unique_ptr<whisper_context, WhisperDeleter>;
struct StateDeleter { void operator()(whisper_state * state) const { whisper_free_state(state); } };
using State = std::unique_ptr<whisper_state, StateDeleter>;
struct LogState { std::atomic<bool> gpu_failed{false}; };
LogState worker_logs; // C側callbackは初期化失敗後も参照可能なprocess寿命の宛先を使う。
void whisper_log(ggml_log_level, const char * text, void * opaque) noexcept {
    auto & log = *static_cast<LogState *>(opaque);
    // v1.9.5はこのGPU失敗後にCPUだけで続ける。固定の発生元だけを監視し、
    // モデルpathに含まれる語は判定に使わない。C callback内で割当てもしない。
    constexpr char failed[] = "whisper_backend_init_gpu: failed to initialize ";
    constexpr char missing[] = "whisper_backend_init_gpu: no GPU found";
    if (std::strncmp(text, failed, sizeof(failed) - 1) == 0 || std::strncmp(text, missing, sizeof(missing) - 1) == 0)
        log.gpu_failed.store(true);
    std::fputs(text, stderr);
}
struct Progress {
    uint64_t request, offset_ms, length_ms;
    std::exception_ptr failure;
    static void update(whisper_context *, whisper_state *, int percent, void * opaque) noexcept {
        auto & p = *static_cast<Progress *>(opaque);
        if (p.failure) return;
        try {
            framing::write(std::cout, {{"type", "progress"}, {"request_id", p.request},
                {"processed_ms", p.offset_ms + p.length_ms * static_cast<uint64_t>(std::clamp(percent, 0, 100)) / 100}});
        } catch (...) { p.failure = std::current_exception(); }
    }
    static bool abort(void * opaque) noexcept { return bool(static_cast<Progress *>(opaque)->failure); }
};
class Engine {
    LogState & logs_ = worker_logs;
    Context context_;
    json device_;
    double load_s_;
    int threads_;
public:
    Engine(const std::string & model, const std::string & device_luid) {
        auto begin = std::chrono::steady_clock::now();
        device_ = select_asr_device(device_luid).description;
        threads_ = static_cast<int>(std::min(8u, std::max(1u, std::thread::hardware_concurrency())));
        whisper_log_set(whisper_log, &logs_);
        auto params = whisper_context_default_params();
        params.use_gpu = true;
        params.gpu_device = 0; // select_asr_deviceが選んだ物理番号だけをVulkan0へ公開済み。
        params.flash_attn = true; // whisper-cli v1.9.5の既定値。
        params.dtw_token_timestamps = false;
        context_.reset(whisper_init_from_file_with_params_no_state(model.c_str(), params));
        if (!context_ || logs_.gpu_failed.load()) throw std::runtime_error("ASR Vulkan model initialization failed");
        load_s_ = std::chrono::duration<double>(std::chrono::steady_clock::now() - begin).count();
    }
    ~Engine() {
        context_.reset();
        whisper_log_set(nullptr, nullptr);
    }
    json loaded() const {
        return {{"type", "load"}, {"status", "ok"}, {"model_load_s", load_s_},
            {"device", device_}, {"cpu_threads", threads_}};
    }
    std::vector<asr::Segment> recognize(const float * pcm, size_t samples, uint64_t offset_ms, uint64_t request) {
        if (!samples) return {};
        // CLI -l ja -t 8 -bs 5 -bo 5 -mc 0と、そのほかのCLI既定値に揃える。
        // 窓ごとに復号stateだけを作り、読込済みの重みは再利用する。
        auto params = whisper_full_default_params(WHISPER_SAMPLING_GREEDY);
        params.strategy = WHISPER_SAMPLING_BEAM_SEARCH;
        params.n_threads = threads_;
        params.language = "ja";
        params.detect_language = false;
        params.translate = false;
        params.n_max_text_ctx = 0;
        // v1.9.5 の既定値 (CLI は上書きしない) を維持する。
        params.greedy.best_of = 5;
        params.beam_search.beam_size = 5;
        params.temperature = 0.0f;
        params.temperature_inc = 0.2f;
        params.offset_ms = 0;
        params.duration_ms = 0;
        params.thold_pt = 0.01f;
        params.max_len = 0;
        params.split_on_word = false;
        params.audio_ctx = 0;
        params.debug_mode = false;
        params.tdrz_enable = false;
        params.suppress_regex = nullptr;
        params.initial_prompt = "";
        params.carry_initial_prompt = false;
        params.entropy_thold = 2.4f;
        params.logprob_thold = -1.0f;
        params.no_speech_thold = 0.6f;
        params.suppress_nst = false;
        params.vad = false;
        params.vad_model_path = nullptr;
        params.token_timestamps = false;
        params.no_timestamps = false;
        params.print_realtime = false;
        params.print_progress = false;
        params.print_timestamps = false;
        params.print_special = false;
        Progress progress{request, offset_ms, asr::sample_ms(samples), {}};
        params.progress_callback = Progress::update;
        params.progress_callback_user_data = &progress;
        params.abort_callback = Progress::abort;
        params.abort_callback_user_data = &progress;
        // モデルの重みだけを再利用し、復号stateはCLIの1回実行と同じく毎窓新規にする。
        State state(whisper_init_state(context_.get()));
        if (!state || logs_.gpu_failed.load()) throw std::runtime_error("ASR Vulkan state initialization failed");
        const int code = whisper_full_with_state(context_.get(), state.get(), params, pcm, static_cast<int>(samples));
        if (progress.failure) std::rethrow_exception(progress.failure);
        if (code != 0 || logs_.gpu_failed.load()) throw std::runtime_error("ASR Vulkan inference failed");
        std::vector<asr::Segment> result;
        for (int i = 0; i < whisper_full_n_segments_from_state(state.get()); ++i) {
            const auto t0 = whisper_full_get_segment_t0_from_state(state.get(), i);
            const auto t1 = whisper_full_get_segment_t1_from_state(state.get(), i);
            if (t0 < 0 || t1 < t0) throw std::runtime_error("Invalid ASR segment timestamps");
            std::string bytes;
            for (int j = 0; j < whisper_full_n_tokens_from_state(state.get(), i); ++j) {
                const auto token = whisper_full_get_token_id_from_state(state.get(), i, j);
                if (token >= whisper_token_eot(context_.get())) continue;
                const char * text = whisper_token_to_str(context_.get(), token);
                if (!text) throw std::runtime_error("Missing ASR token bytes");
                bytes += text;
            }
            result.push_back({static_cast<uint64_t>(t0) * 10, static_cast<uint64_t>(t1) * 10, std::move(bytes)});
        }
        return result;
    }
};
}

int asr_serve() {
    if (_setmode(_fileno(stdin), _O_BINARY) == -1 || _setmode(_fileno(stdout), _O_BINARY) == -1)
        throw std::runtime_error("Cannot put ASR protocol pipes in binary mode");
    asr_protocol::Session session;
    std::unique_ptr<Engine> engine;
    std::unique_ptr<asr::Stream> stream;
    framing::Frame frame;
    while (framing::read(std::cin, frame)) {
        const auto kind = asr_protocol::validate(frame);
        session.accept(kind, frame.header, frame.bytes.size());
        if (kind == asr_protocol::Kind::Shutdown) return 0;
        if (kind == asr_protocol::Kind::Hello) {
            framing::write(std::cout, {{"type", "hello"}, {"protocol_version", asr_protocol::kVersion},
                {"role", "asr"}, {"settings_version", asr_protocol::kSettingsVersion}, {"source_identity", MIV_VLM_SOURCE_SHA256}});
            continue;
        }
        try {
            if (kind == asr_protocol::Kind::Load) {
                engine = std::make_unique<Engine>(framing::string_field(frame.header, "model_path"),
                    framing::string_field(frame.header, "device_luid"));
                framing::write(std::cout, engine->loaded());
                continue;
            }
            const auto request = framing::uint_field(frame.header, "request_id");
            if (kind == asr_protocol::Kind::Begin) {
                stream = std::make_unique<asr::Stream>([&engine, request](const float * pcm, size_t size, uint64_t offset) {
                    return engine->recognize(pcm, size, offset, request);
                }, [request, &session](asr::Window window) {
                    json segments = json::array();
                    for (const auto & s : window.segments)
                        segments.push_back({{"start_ms", s.start_ms}, {"end_ms", s.end_ms}, {"text", s.text}});
                    session.record_window(window.utf8_discarded_bytes, window.utf8_discarded_fragments);
                    framing::write(std::cout, {{"type", "window"}, {"request_id", request}, {"start_ms", window.start_ms},
                        {"read_end_ms", window.read_end_ms}, {"join_ms", window.join_ms}, {"final", window.final},
                        {"segments", std::move(segments)}, {"utf8_discarded_bytes", window.utf8_discarded_bytes},
                        {"utf8_discarded_fragments", window.utf8_discarded_fragments}});
                });
                framing::write(std::cout, {{"type", "begin"}, {"request_id", request}, {"status", "ok"}});
            } else if (kind == asr_protocol::Kind::Pcm) {
                stream->append(frame.bytes.data(), frame.bytes.size());
            } else if (kind == asr_protocol::Kind::End) {
                stream->end();
                framing::write(std::cout, session.end_response());
                stream.reset();
                session.complete_end();
            }
        } catch (const std::exception & error) {
            json response{{"type", kind == asr_protocol::Kind::Load ? "load" : "error"},
                {"status", "fatal"}, {"code", kind == asr_protocol::Kind::Load ? "model_load" : "inference"},
                {"error", error.what()}};
            if (frame.header.contains("request_id")) response["request_id"] = frame.header["request_id"];
            framing::write(std::cout, response);
            return 1;
        }
    }
    session.accept_eof();
    return 0;
}
