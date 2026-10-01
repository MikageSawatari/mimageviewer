#include "vcrt_preload.h"
#include "utf8_paths.h"
#include <cstdio>
#include <vector>

namespace miv {
namespace {
std::filesystem::path executable_directory() {
    std::vector<wchar_t> buffer(32768);
    const DWORD length = GetModuleFileNameW(nullptr, buffer.data(), static_cast<DWORD>(buffer.size()));
    if (!length || length >= buffer.size()) throw std::runtime_error("Cannot resolve host executable path");
    return std::filesystem::path(std::wstring(buffer.data(), length)).parent_path();
}
std::filesystem::path system_directory() {
    std::vector<wchar_t> buffer(32768);
    const UINT length = GetSystemDirectoryW(buffer.data(), static_cast<UINT>(buffer.size()));
    if (!length || length >= buffer.size()) throw std::runtime_error("Cannot resolve System32 path");
    return std::filesystem::path(std::wstring(buffer.data(), length));
}
} // namespace

bool preload_vcrt() {
    try {
        const auto system = system_directory();
        const auto bundled = executable_directory() / L"vcrt";
        // Dependencies are pinned first; no SetDllDirectory/AddDllDirectory or ACP
        // manifest changes affect the existing plugin's search/code-page behavior.
        for (const auto* name : {L"vcruntime140.dll", L"vcruntime140_1.dll", L"msvcp140.dll", L"msvcp140_1.dll"}) {
            const auto system_path = system / name;
            const DWORD attributes = GetFileAttributesW(system_path.c_str());
            const DWORD error = attributes == INVALID_FILE_ATTRIBUTES ? GetLastError() : ERROR_SUCCESS;
            const auto location = vcrt_location(attributes != INVALID_FILE_ATTRIBUTES, error);
            if (location == VcrtLocation::Error) {
                std::fprintf(stderr, "[BRIDGE CRT] System32 stat failed: %s error=%lu\n", path_to_utf8(system_path).c_str(), error);
                return false;
            }
            const auto path = location == VcrtLocation::System32 ? system_path : bundled / name;
            // Absolute path plus restricted dependency lookup prevents legacy
            // CRTs beside the old host or in CWD/PATH from overriding the choice.
            // Dependencies were explicitly selected in the order above. Do not
            // search vcrt/ implicitly: that could load an unselected companion
            // before its System32-first decision.
            const DWORD flags = LOAD_LIBRARY_SEARCH_SYSTEM32;
            const auto module = LoadLibraryExW(path.c_str(), nullptr, flags);
            if (!module) {
                const DWORD load_error = GetLastError();
                std::fprintf(stderr, "[BRIDGE CRT] preload failed source=%s path=%s error=%lu\n",
                    location == VcrtLocation::System32 ? "System32" : "bundled", path_to_utf8(path).c_str(), load_error);
                return false;
            }
            // Keep the reference for the lifetime of the host process. Subsequent
            // plugin imports use the preloaded DLL with this basename.
            std::fprintf(stderr, "[BRIDGE CRT] source=%s path=%s\n",
                location == VcrtLocation::System32 ? "System32" : "bundled", path_to_utf8(path).c_str());
        }
        std::fflush(stderr);
        return true;
    } catch (const std::exception& error) {
        std::fprintf(stderr, "[BRIDGE CRT] preparation failed: %s\n", error.what());
        return false;
    }
}
} // namespace miv
