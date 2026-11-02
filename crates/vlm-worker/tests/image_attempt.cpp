#include "engine_errors.h"
#include "image_input.h"
#include <iostream>
#include <limits>
#include <new>

int main() {
    int failures = 0;
    auto check = [&](bool value) { if (!value) ++failures; };
    int completed = 0;
    EngineErrors errors;
    for (int stage = 0; stage < 5; ++stage) {
        nlohmann::json record;
        run_image_attempt(record, [&] {
            if (stage == 0) errors.check_tokenize(2);
            if (stage == 1) check_image_context_budget(8192 - 800 + 1);
            if (stage == 2) errors.check_evaluation(1, "image prompt");
            if (stage == 3) errors.check_evaluation(-1, "image generation");
            ++completed;
            record["status"] = "ok";
        });
        check(record["status"] == (stage == 4 ? "ok" : "failed"));
        if (stage < 4) check(!record["validation"]["ok"].get<bool>() && record["finish_reason"] == "error");
    }
    check(completed == 1); // Each failed image permits the next attempt.
    check_image_context_budget(8192 - 800);
    try { check_image_context_budget((std::numeric_limits<size_t>::max)()); check(false); } catch (const ImageFailure &) {}
    for (auto size : {std::pair<uint32_t, uint32_t>{131072, 1}, {1, 131072}, {0, 1}, {1025, 1}}) {
        try { check_image_dimensions(size.first, size.second); check(false); } catch (const ImageFailure &) {}
    }
    check_image_dimensions(1024, 1024);
    for (int code : {-2, -3}) {
        try {
            nlohmann::json record;
            run_image_attempt(record, [&] { errors.check_evaluation(code, "fatal decode"); });
            check(false);
        } catch (const ImageFailure &) { check(false); } catch (const std::runtime_error &) {}
    }
    for (const char * diagnostic : {"clip_encode: failed to allocate compute graph\n", "mtmd_tokenize: error: bad allocation\n",
            "mtmd_encode_chunk: error: vk::Device::allocateMemory: ErrorOutOfDeviceMemory\n",
            "mtmd_encode_chunk: error: vk::Queue::submit: ErrorDeviceLost\n",
            "clip_encode: ggml_backend_sched_graph_compute failed with error -2\n",
            "mtmd_encode_chunk: error: Requested buffer size exceeds device buffer size limit: ErrorOutOfDeviceMemory\n"}) {
        EngineErrors resource;
        resource.observe(GGML_LOG_LEVEL_ERROR, diagnostic);
        try {
            nlohmann::json record;
            run_image_attempt(record, [&] { resource.check_tokenize(2); });
            check(false);
        } catch (const ImageFailure &) { check(false); } catch (const std::runtime_error &) {}
    }
    for (auto level : {GGML_LOG_LEVEL_NONE, GGML_LOG_LEVEL_DEBUG, GGML_LOG_LEVEL_INFO, GGML_LOG_LEVEL_WARN, GGML_LOG_LEVEL_CONT}) {
        EngineErrors normal;
        normal.observe(level, "load_model: G:\\models\\bad_alloc\\model.gguf\n");
        normal.observe(level, "clip_model_load: G:\\out of memory\\mmproj.gguf\n");
        normal.observe(level, "clip_encode: failed to allocate compute graph\n");
        check(!normal.fatal.load());
        normal.check_tokenize(0);
    }
    for (const char * normal : {"load_model: G:\\models\\bad_alloc\\model.gguf\n",
            "clip_model_load: G:\\out of memory\\mmproj.gguf\n",
            "metadata: clip_encode: failed to allocate compute graph\n",
            "mtmd_tokenize: error: unsupported metadata G:\\models\\bad_alloc\\model.gguf\n",
            "mtmd_encode_chunk: error: cannot open G:\\out of memory\\ErrorOutOfDeviceMemory\n",
            "clip_encode: failed to allocate compute graph\nextra metadata",
            "mtmd_encode_chunk: error: vk::Device::allocateMemory: ErrorOutOfDeviceMemory\nextra metadata",
            "clip_encode: ggml_backend_sched_graph_compute failed with error 1\n"}) {
        EngineErrors errors_with_metadata;
        errors_with_metadata.observe(GGML_LOG_LEVEL_ERROR, normal);
        check(!errors_with_metadata.fatal.load());
    }
    try {
        nlohmann::json record;
        run_image_attempt(record, [] { throw std::bad_alloc(); });
        check(false);
    } catch (const std::bad_alloc &) {}
    std::cout << "image failure boundary failures=" << failures << '\n';
    return failures ? 1 : 0;
}
