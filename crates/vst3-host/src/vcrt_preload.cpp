#include "vcrt_preload.h"
#include "utf8_paths.h"
#include <winver.h>
#include <cstdio>
#include <vector>

namespace miv {
namespace {
std::filesystem::path executable_directory() {
    std::vector<wchar_t> buffer(32768);
    const DWORD length = GetModuleFileNameW(nullptr, buffer.data(), static_cast<DWORD>(buffer.size()));
    if (!length || length >= buffer.size()) throw std::runtime_error("Cannot resolve host executable path");
    return native_windows_path(std::wstring(buffer.data(), length)).parent_path();
}
std::filesystem::path system_directory() {
    std::vector<wchar_t> buffer(32768);
    const UINT length = GetSystemDirectoryW(buffer.data(), static_cast<UINT>(buffer.size()));
    if (!length || length >= buffer.size()) throw std::runtime_error("Cannot resolve System32 path");
    return native_windows_path(std::wstring(buffer.data(), length));
}
} // namespace

namespace {
VcrtFile file_version(const std::filesystem::path& path) {
    DWORD ignored = 0;
    const DWORD size = GetFileVersionInfoSizeW(path.c_str(), &ignored);
    if (!size) return {};
    std::vector<unsigned char> data(size);
    if (!GetFileVersionInfoW(path.c_str(), 0, size, data.data())) return {};
    VS_FIXEDFILEINFO* information = nullptr;
    UINT length = 0;
    if (!VerQueryValueW(data.data(), L"\\", reinterpret_cast<void**>(&information), &length) ||
        length < sizeof(VS_FIXEDFILEINFO) || !information || information->dwSignature != 0xfeef04bd)
        return {};
    return {true, {
        HIWORD(information->dwFileVersionMS), LOWORD(information->dwFileVersionMS),
        HIWORD(information->dwFileVersionLS), LOWORD(information->dwFileVersionLS)}};
}
void log_version(const char* source, const std::filesystem::path& path, VcrtFile file) {
    const auto version = file.version;
    std::fprintf(stderr, "[BRIDGE CRT] candidate=%s readable=%u version=%u.%u.%u.%u path=%s\n",
        source, static_cast<unsigned>(file.readable), static_cast<unsigned>(version.major), static_cast<unsigned>(version.minor), static_cast<unsigned>(version.build),
        static_cast<unsigned>(version.revision), path_to_utf8(path).c_str());
}
} // namespace

bool preload_vcrt() {
    try {
        const auto system = system_directory();
        const auto bundled = executable_directory() / L"vcrt";
        constexpr std::array<const wchar_t*, 4> names{
            L"vcruntime140.dll", L"vcruntime140_1.dll", L"msvcp140.dll", L"msvcp140_1.dll"};
        VcrtSet system_set{}, bundled_set{};
        for (size_t index = 0; index < names.size(); ++index) {
            system_set[index] = file_version(system / names[index]);
            bundled_set[index] = file_version(bundled / names[index]);
            log_version("System32", system / names[index], system_set[index]);
            log_version("bundled", bundled / names[index], bundled_set[index]);
        }
        const auto location = choose_vcrt_set(system_set, bundled_set);
        if (location == VcrtLocation::Error) {
            std::fprintf(stderr, "[BRIDGE CRT] bundled set is missing, unreadable, or has inconsistent versions\n");
            return false;
        }
        const auto& directory = location == VcrtLocation::System32 ? system : bundled;
        const char* source = location == VcrtLocation::System32 ? "System32" : "bundled";
        // Preload the selected whole set in dependency order. Keep references
        // until process exit. All dependencies are already explicitly selected;
        // restrict any remaining lookup to System32, never CWD/PATH/legacy host.
        for (size_t index = 0; index < names.size(); ++index) {
            const auto path = directory / names[index];
            const auto module = LoadLibraryExW(path.c_str(), nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32);
            if (!module) {
                const DWORD error = GetLastError();
                std::fprintf(stderr, "[BRIDGE CRT] preload failed source=%s path=%s error=%lu\n",
                    source, path_to_utf8(path).c_str(), error);
                return false; // Never switch source halfway through the set.
            }
            const auto version = location == VcrtLocation::System32 ? system_set[index].version : bundled_set[index].version;
            std::fprintf(stderr, "[BRIDGE CRT] source=%s version=%u.%u.%u.%u path=%s\n",
                source, static_cast<unsigned>(version.major), static_cast<unsigned>(version.minor), static_cast<unsigned>(version.build), static_cast<unsigned>(version.revision), path_to_utf8(path).c_str());
        }
        std::fflush(stderr);
        return true;
    } catch (const std::exception& error) {
        std::fprintf(stderr, "[BRIDGE CRT] preparation failed: %s\n", error.what());
        return false;
    }
}
} // namespace miv
