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
    for (const char * diagnostic : {"clip: failed to allocate compute graph\n", "mtmd_tokenize: error: bad allocation\n",
            "mtmd_encode_chunk: vk::Device::allocateMemory: ErrorOutOfDeviceMemory\n", "device lost"}) {
        EngineErrors resource;
        resource.observe(diagnostic);
        try {
            nlohmann::json record;
            run_image_attempt(record, [&] { resource.check_tokenize(2); });
            check(false);
        } catch (const ImageFailure &) { check(false); } catch (const std::runtime_error &) {}
    }
    try {
        nlohmann::json record;
        run_image_attempt(record, [] { throw std::bad_alloc(); });
        check(false);
    } catch (const std::bad_alloc &) {}
    std::cout << "image failure boundary failures=" << failures << '\n';
    return failures ? 1 : 0;
}
