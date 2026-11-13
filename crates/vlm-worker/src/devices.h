#pragma once
#include "ggml-backend.h"
#include <nlohmann/json.hpp>
#include <string>

struct DeviceSelection {
    ggml_backend_dev_t device;
    nlohmann::json description;
};
nlohmann::json list_devices();
DeviceSelection select_device(const std::string & luid);
// ASRは選択済みLUIDを最新の共通一覧へ直接照合し、内蔵GPUも候補にする。
DeviceSelection select_asr_device(const std::string & luid);
