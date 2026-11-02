#pragma once
#include "image_attempt.h"
#include <atomic>
#include <cstring>

// b12032 mtmd catches allocation exceptions and returns the same code as bad images.
struct EngineErrors {
    std::atomic<bool> fatal{false};
    void observe(const char * message) noexcept {
        for (const char * marker : {"bad allocation", "bad_alloc", "failed to allocate", "allocation failed",
                "out of memory", "OutOfDeviceMemory", "OutOfHostMemory", "ErrorDeviceLost", "device lost"}) {
            if (std::strstr(message, marker)) fatal.store(true, std::memory_order_relaxed);
        }
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
