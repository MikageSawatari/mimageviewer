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
