#pragma once
#include "devices.h"
#include "engine_errors.h"
#include "llama.h"
#include "mtmd-helper.h"
#include <chrono>
#include <filesystem>
#include <memory>

template<class T, void (*Free)(T *)> using Handle = std::unique_ptr<T, decltype(Free)>;
using Clock = std::chrono::steady_clock;
inline double seconds(Clock::time_point start) { return std::chrono::duration<double>(Clock::now() - start).count(); }

// Own only reusable weights. Context, sequence, bitmap and sampler are per image.
class InferenceEngine {
    EngineErrors errors_;
    DeviceSelection selected_;
    int threads_;
    Handle<llama_model, llama_model_free> model_{nullptr, llama_model_free};
    Handle<mtmd_context, mtmd_free> vision_{nullptr, mtmd_free};
    double load_s_ = 0;
public:
    InferenceEngine(const std::string & model_path, const std::string & mmproj_path, const std::string & luid, int threads);
    const nlohmann::json & device() const { return selected_.description; }
    double load_seconds() const { return load_s_; }
    void caption_rgb(nlohmann::json & record, uint32_t width, uint32_t height, const unsigned char * rgb, const char * bitmap_id = nullptr);
    void caption_png(nlohmann::json & record, const std::filesystem::path & path);
};
void route_engine_logs_to_stderr();
