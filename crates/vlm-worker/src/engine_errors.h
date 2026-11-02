#pragma once
#include "image_attempt.h"
#include "ggml.h"
#include <atomic>
#include <string_view>

// b12032 mtmd catches allocation exceptions and returns the same code as bad images.
struct EngineErrors {
    std::atomic<bool> fatal{false};
    static bool vulkan_resource_exception(std::string_view what) noexcept {
        auto delimiter = what.rfind(": ");
        if (delimiter == std::string_view::npos) return false;
        auto origin = what.substr(0, delimiter);
        auto code = what.substr(delimiter + 2);
        if (code != "ErrorOutOfDeviceMemory" && code != "ErrorOutOfHostMemory" && code != "ErrorDeviceLost") return false;
        if (origin == "Requested buffer size exceeds device buffer size limit" || origin == "No suitable memory type found")
            return code == "ErrorOutOfDeviceMemory";
        // Vulkan-Hpp generated exception origin: a qualified API identifier, never a path.
        if (origin.substr(0, 4) != "vk::") return false;
        origin.remove_prefix(4);
        auto separator = origin.find("::");
        if (separator == std::string_view::npos) return false;
        auto identifier = [](std::string_view name) {
            auto first = [](char c) { return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c == '_'; };
            if (name.empty() || !first(name.front())) return false;
            for (char c : name) if (!first(c) && !(c >= '0' && c <= '9')) return false;
            return true;
        };
        return identifier(origin.substr(0, separator)) && identifier(origin.substr(separator + 2));
    }
    // Called only by mtmd/clip's callback. llama errors use their typed return codes.
    void observe(ggml_log_level level, const char * message) noexcept {
        // CONT has a larger numeric value but is not a higher severity.
        if (level != GGML_LOG_LEVEL_ERROR || !message) return;
        std::string_view text(message);
        bool resource = text == "clip_encode: failed to allocate compute graph\n"
            || text == "clip_gen_state_init: failed to allocate the gen state\n"
            || text == "clip_enc_state_init: failed to allocate the encoder state\n"
            || text == "clip_encode: ggml_backend_sched_graph_compute failed with error -2\n"
            || text == "clip_encode: ggml_backend_sched_graph_compute failed with error -1\n";
        for (auto prefix : {std::string_view("mtmd_tokenize: error: "), std::string_view("mtmd_encode_chunk: error: ")}) {
            if (text.substr(0, prefix.size()) != prefix || text.empty() || text.back() != '\n') continue;
            auto what = text.substr(prefix.size(), text.size() - prefix.size() - 1);
            resource = resource || what == "bad allocation" || what == "std::bad_alloc" || vulkan_resource_exception(what);
        }
        if (resource) fatal.store(true, std::memory_order_relaxed);
    }
    void check_fatal() const {
        if (fatal.load(std::memory_order_relaxed)) throw std::runtime_error("Engine memory allocation or device failure");
    }
    void check_tokenize(int result) const {
        check_fatal();
        if (result != 0) throw ImageFailure("Image preprocessing/tokenization failed");
    }
    void check_evaluation(int result, const char * stage) const {
        check_fatal();
        // llama_process: -1 invalid batch; <-1 fatal; positive codes are nonfatal.
        if (result < -1) throw std::runtime_error(std::string(stage) + " (fatal engine error)");
        if (result != 0) throw ImageFailure(stage);
    }
};
