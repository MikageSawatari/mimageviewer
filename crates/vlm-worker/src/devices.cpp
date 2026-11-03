#include "devices.h"
#include "llama.h"
#include <windows.h>
#include <dxgi1_2.h>
#include <vulkan/vulkan.h>
#include <algorithm>
#include <cctype>
#include <cstdio>
#include <cstring>
#include <stdexcept>
#include <vector>

using json = nlohmann::json;
namespace {
std::string luid_string(const void * bytes) {
    LUID luid;
    std::memcpy(&luid, bytes, sizeof(luid));
    char buf[18];
    std::snprintf(buf, sizeof(buf), "%08X:%08X", static_cast<unsigned>(luid.HighPart), luid.LowPart);
    return buf;
}
std::string utf8(const wchar_t * text) {
    int n = WideCharToMultiByte(CP_UTF8, 0, text, -1, nullptr, 0, nullptr, nullptr);
    if (n <= 0) throw std::runtime_error("Adapter name conversion failed");
    std::string result(n, '\0');
    WideCharToMultiByte(CP_UTF8, 0, text, -1, result.data(), n, nullptr, nullptr);
    result.pop_back();
    return result;
}
struct Inventory {
    json dxgi = json::array();
    json vulkan = json::array();
    std::string vulkan_error;
    explicit Inventory(bool allow_unavailable = false) {
        IDXGIFactory1 * factory = nullptr;
        if (FAILED(CreateDXGIFactory1(IID_PPV_ARGS(&factory)))) throw std::runtime_error("DXGI factory failed");
        for (UINT i = 0;; ++i) {
            IDXGIAdapter1 * adapter = nullptr;
            HRESULT hr = factory->EnumAdapters1(i, &adapter);
            if (hr == DXGI_ERROR_NOT_FOUND) break;
            if (FAILED(hr)) { factory->Release(); throw std::runtime_error("DXGI enumeration failed"); }
            DXGI_ADAPTER_DESC1 d{};
            hr = adapter->GetDesc1(&d);
            adapter->Release();
            if (FAILED(hr)) { factory->Release(); throw std::runtime_error("DXGI description failed"); }
            dxgi.push_back({{"index", i}, {"name", utf8(d.Description)}, {"luid", luid_string(&d.AdapterLuid)},
                {"dedicated_bytes", d.DedicatedVideoMemory}, {"software", bool(d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE)}});
        }
        factory->Release();
        try {
            VkApplicationInfo app{VK_STRUCTURE_TYPE_APPLICATION_INFO};
            if (vkEnumerateInstanceVersion(&app.apiVersion) != VK_SUCCESS || app.apiVersion < VK_API_VERSION_1_2)
                throw std::runtime_error("Vulkan 1.2 or later is required");
            VkInstanceCreateInfo ci{VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO};
            ci.pApplicationInfo = &app;
            VkInstance instance{};
            if (vkCreateInstance(&ci, nullptr, &instance) != VK_SUCCESS) throw std::runtime_error("Vulkan instance failed");
            try {
                uint32_t count = 0;
                if (vkEnumeratePhysicalDevices(instance, &count, nullptr) != VK_SUCCESS) throw std::runtime_error("Vulkan enumeration failed");
                std::vector<VkPhysicalDevice> devices(count);
                if (vkEnumeratePhysicalDevices(instance, &count, devices.data()) != VK_SUCCESS) throw std::runtime_error("Vulkan enumeration failed");
                for (uint32_t i = 0; i < count; ++i) {
                    VkPhysicalDeviceIDProperties id{VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_ID_PROPERTIES};
                    VkPhysicalDeviceProperties2 p{VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2};
                    p.pNext = &id;
                    vkGetPhysicalDeviceProperties2(devices[i], &p);
                    json row{{"physical_index", i}, {"name", p.properties.deviceName}, {"luid_valid", bool(id.deviceLUIDValid)},
                        {"luid", id.deviceLUIDValid ? luid_string(id.deviceLUID) : ""}};
                    uint32_t n_ext = 0;
                    if (vkEnumerateDeviceExtensionProperties(devices[i], nullptr, &n_ext, nullptr) != VK_SUCCESS)
                        throw std::runtime_error("Vulkan extension enumeration failed");
                    std::vector<VkExtensionProperties> extensions(n_ext);
                    if (vkEnumerateDeviceExtensionProperties(devices[i], nullptr, &n_ext, extensions.data()) != VK_SUCCESS)
                        throw std::runtime_error("Vulkan extension enumeration failed");
                    for (const auto & extension : extensions) if (std::strcmp(extension.extensionName, "VK_EXT_pci_bus_info") == 0) {
                        VkPhysicalDevicePCIBusInfoPropertiesEXT pci{VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PCI_BUS_INFO_PROPERTIES_EXT};
                        p.pNext = &pci;
                        vkGetPhysicalDeviceProperties2(devices[i], &p);
                        char bus[32];
                        std::snprintf(bus, sizeof(bus), "%04x:%02x:%02x.%x", pci.pciDomain, pci.pciBus, pci.pciDevice, pci.pciFunction);
                        row["pci_id"] = bus;
                    }
                    for (auto & d : dxgi) if (id.deviceLUIDValid && d["luid"] == row["luid"]) row["dxgi"] = d;
                    vulkan.push_back(row);
                }
            } catch (...) { vkDestroyInstance(instance, nullptr); throw; }
            vkDestroyInstance(instance, nullptr);
        } catch (const std::exception & error) {
            if (!allow_unavailable) throw;
            vulkan = json::array();
            vulkan_error = error.what();
        }
    }
};
json ggml_inventory() {
    llama_backend_init();
    json out = json::array();
    for (size_t i = 0; i < ggml_backend_dev_count(); ++i) {
        auto dev = ggml_backend_dev_get(i);
        ggml_backend_dev_props p{};
        ggml_backend_dev_get_props(dev, &p);
        out.push_back({{"index", i}, {"name", p.name}, {"description", p.description},
            {"backend", ggml_backend_reg_name(ggml_backend_dev_backend_reg(dev))},
            {"device_id", p.device_id ? p.device_id : ""}, {"memory_total", p.memory_total}});
    }
    return out;
}
}

json list_devices() {
    Inventory inventory(true);
    // Ignore inherited filtering so this diagnostic always shows the full set.
    if (!SetEnvironmentVariableA("GGML_VK_VISIBLE_DEVICES", nullptr)) throw std::runtime_error("Cannot clear Vulkan filter");
    if (_putenv_s("GGML_VK_VISIBLE_DEVICES", "") != 0) throw std::runtime_error("Cannot clear CRT Vulkan filter");
    json ggml = inventory.vulkan.empty() ? json::array() : ggml_inventory();
    for (auto & v : inventory.vulkan) {
        json matches = json::array();
        for (auto & g : ggml) if (g["backend"] == "Vulkan" &&
            (v.contains("pci_id") ? g["device_id"] == v["pci_id"] : g["description"] == v["name"])) matches.push_back(g["name"]);
        size_t same_name = 0;
        for (auto & other : inventory.vulkan) if (other["name"] == v["name"]) ++same_name;
        v["ggml_name_candidates"] = matches;
        v["mapping"] = matches.size() == 1 && (v.contains("pci_id") || same_name == 1)
            ? (v.contains("pci_id") ? "pci-id" : "unique-description") : "ambiguous/unavailable";
    }
    return {{"dxgi", inventory.dxgi}, {"vulkan", inventory.vulkan}, {"ggml", ggml},
        {"vulkan_available", !inventory.vulkan.empty()}, {"vulkan_error", inventory.vulkan_error},
        {"selection_method", "DXGI LUID -> Vulkan physical index -> GGML_VK_VISIBLE_DEVICES before backend initialization -> sole Vulkan0"}};
}

DeviceSelection select_device(const std::string & requested) {
    Inventory inventory;
    json selected;
    size_t largest = 0;
    std::string normalized = requested;
    std::transform(normalized.begin(), normalized.end(), normalized.begin(), [](unsigned char c) { return char(std::toupper(c)); });
    for (auto & v : inventory.vulkan) {
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
    for (const auto & v : inventory.vulkan) if (v["name"] == selected["name"]) ++same_name;
    if (!selected.contains("pci_id") && same_name != 1)
        throw std::runtime_error("Cannot verify same-name GPUs without PCI identity; ggml exposes no LUID/UUID API");
    std::string index = std::to_string(selected["physical_index"].get<uint32_t>());
    if (!SetEnvironmentVariableA("GGML_VK_VISIBLE_DEVICES", index.c_str())) throw std::runtime_error("Cannot pin Vulkan physical device");
    // ggml uses the CRT getenv(), so update its environment as well.
    if (_putenv_s("GGML_VK_VISIBLE_DEVICES", index.c_str()) != 0) throw std::runtime_error("Cannot pin CRT Vulkan device");
    llama_backend_init();
    auto reg = ggml_backend_reg_by_name("Vulkan");
    if (!reg || ggml_backend_reg_dev_count(reg) != 1) throw std::runtime_error("Pinned Vulkan backend must expose exactly one device");
    auto dev = ggml_backend_reg_dev_get(reg, 0);
    if (selected["name"] != ggml_backend_dev_description(dev)) throw std::runtime_error("Pinned Vulkan device description mismatch");
    if (selected.contains("pci_id")) {
        ggml_backend_dev_props props{};
        ggml_backend_dev_get_props(dev, &props);
        if (!props.device_id || selected["pci_id"] != props.device_id) throw std::runtime_error("Pinned Vulkan PCI identity mismatch");
    }
    selected["ggml_name"] = ggml_backend_dev_name(dev);
    selected["selection_method"] = "physical-index visibility filter before ggml init";
    selected["identity_check"] = selected.contains("pci_id") ? "pci-id" : "unique-description";
    return {dev, selected};
}
