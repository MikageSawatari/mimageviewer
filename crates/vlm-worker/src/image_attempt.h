#pragma once
#include "contracts.h"
#include <stdexcept>
#include <string>

struct ImageFailure : std::runtime_error { using std::runtime_error::runtime_error; };

inline void check_image_context_budget(size_t tokens) {
    if (tokens > 8192 - 800) throw ImageFailure("Prompt exceeds context budget");
}

template<class Work> void run_image_attempt(nlohmann::json & record, Work && work) {
    try {
        work();
    } catch (const ImageFailure & error) {
        record["error"] = error.what();
        record["finish_reason"] = "error";
        record["validation"] = validate_output("", false, 0);
        record["json_ok"] = false;
        record["status"] = "failed";
    }
}
