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
// ASRは共通一覧の物理Vulkan番号を受け取り、内蔵GPUも候補にする。
// ggmlの最初の初期化より前に、そのデバイスだけを公開する。
DeviceSelection select_asr_device(uint32_t physical_index);
