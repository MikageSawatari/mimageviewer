#pragma once

// IPC and SDK strings are UTF-8. Convert explicitly at every Windows boundary;
// /utf-8 does not change filesystem::path's narrow constructor or the process ACP.
#include <windows.h>
#include <filesystem>
#include <limits>
#include <stdexcept>
#include <string>
#include <string_view>

namespace miv {
inline std::wstring utf8_to_utf16(std::string_view text) {
    if (text.empty()) return {};
    if (text.size() > static_cast<size_t>((std::numeric_limits<int>::max)()) ||
        text.find('\0') != std::string_view::npos)
        throw std::runtime_error("Invalid UTF-8 Windows string length or embedded NUL");
    const int count = static_cast<int>(text.size());
    const int needed = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, text.data(), count, nullptr, 0);
    if (needed == 0) throw std::runtime_error("Invalid UTF-8 Windows string");
    std::wstring out(static_cast<size_t>(needed), L'\0');
    if (MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, text.data(), count, out.data(), needed) != needed)
        throw std::runtime_error("UTF-8 to UTF-16 conversion failed");
    return out;
}

inline std::string utf16_to_utf8(std::wstring_view text) {
    if (text.empty()) return {};
    if (text.size() > static_cast<size_t>((std::numeric_limits<int>::max)()))
        throw std::runtime_error("Invalid UTF-16 string length");
    const int count = static_cast<int>(text.size());
    const int needed = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, text.data(), count, nullptr, 0, nullptr, nullptr);
    if (needed == 0) throw std::runtime_error("Invalid UTF-16 Windows string");
    std::string out(static_cast<size_t>(needed), '\0');
    if (WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, text.data(), count, out.data(), needed, nullptr, nullptr) != needed)
        throw std::runtime_error("UTF-16 to UTF-8 conversion failed");
    return out;
}

inline std::filesystem::path path_from_utf8(std::string_view text) {
    return std::filesystem::path(utf8_to_utf16(text));
}
inline std::string path_to_utf8(const std::filesystem::path& path) {
    return utf16_to_utf8(path.generic_wstring());
}
inline std::filesystem::path bundle_binary_path(std::string_view text, std::string_view architecture) {
    const auto bundle = path_from_utf8(text);
    return bundle / L"Contents" / path_from_utf8(architecture) / bundle.filename();
}
// Factory/editor display text belongs to third-party plugins. A malformed title
// must not escape a Win32 paint callback or terminate an otherwise usable host.
// Paths and IPC resource names continue to use the strict conversion above.
inline std::wstring display_title_from_utf8(std::string_view text) {
    if (text.empty()) return L"VST3 Plugin";
    try { return utf8_to_utf16(text); }
    catch (const std::runtime_error&) { return L"VST3 Plugin"; }
}
} // namespace miv
