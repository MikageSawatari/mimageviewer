#pragma once
#include <nlohmann/json.hpp>
#include <algorithm>
#include <cctype>
#include <stdexcept>
#include <string>

namespace worker_devices {
// OS列挙とbackend初期化から分けた選択部分。LUIDを最新の一覧に直接照合する。
// 空LUIDでの既存VLM選択 (専用メモリ最大、0のみなら不採用) は変えない。
inline nlohmann::json select_inventory_device(const nlohmann::json & vulkan, const std::string & requested) {
    nlohmann::json selected;
    size_t largest = 0;
    std::string normalized = requested;
    std::transform(normalized.begin(), normalized.end(), normalized.begin(), [](unsigned char c) { return char(std::toupper(c)); });
    for (const auto & v : vulkan) {
        if (!v.contains("dxgi") || v["dxgi"]["software"].get<bool>()) continue;
        auto bytes = v["dxgi"]["dedicated_bytes"].get<size_t>();
        if ((!normalized.empty() && v["luid"] == normalized) || (normalized.empty() && bytes > largest)) {
            if (!normalized.empty() && !selected.is_null()) throw std::runtime_error("Multiple Vulkan devices have the requested LUID");
            selected = v;
            largest = bytes;
        }
    }
    if (selected.is_null()) throw std::runtime_error("No matching DXGI/Vulkan adapter with valid LUID");
    size_t same_name = 0;
    for (const auto & v : vulkan) if (v["name"] == selected["name"]) ++same_name;
    if (!selected.contains("pci_id") && same_name != 1)
        throw std::runtime_error("Cannot verify same-name GPUs without PCI identity; ggml exposes no LUID/UUID API");
    return selected;
}
}
