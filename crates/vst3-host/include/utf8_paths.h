#pragma once

// IPC and SDK strings are UTF-8. Convert explicitly at every Windows boundary;
// /utf-8 does not change filesystem::path's narrow constructor or the process ACP.
#include <windows.h>
#include <algorithm>
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

inline std::filesystem::path native_windows_path(std::wstring text) {
    if (text.empty()) throw std::runtime_error("Empty Windows path");
    std::replace(text.begin(), text.end(), L'/', L'\\');
    if (text.rfind(LR"(\\?\)", 0) == 0) return std::filesystem::path(text);
    const DWORD needed = GetFullPathNameW(text.c_str(), 0, nullptr, nullptr);
    if (!needed) throw std::runtime_error("Cannot resolve absolute Windows path");
    std::wstring absolute(needed, L'\0');
    const DWORD length = GetFullPathNameW(text.c_str(), needed, absolute.data(), nullptr);
    if (!length || length >= needed) throw std::runtime_error("Cannot resolve absolute Windows path");
    absolute.resize(length);
    // Extended local/UNC syntax works without a longPathAware manifest or a
    // machine-wide registry setting. Native backslashes must remain intact.
    if (absolute.rfind(LR"(\\)", 0) == 0)
        absolute = LR"(\\?\UNC\)" + absolute.substr(2);
    else
        absolute = LR"(\\?\)" + absolute;
    return std::filesystem::path(absolute);
}
inline std::filesystem::path path_from_utf8(std::string_view text) {
    return native_windows_path(utf8_to_utf16(text));
}
inline std::string path_to_utf8(const std::filesystem::path& path) {
    return utf16_to_utf8(path.native());
}
inline std::filesystem::path bundle_binary_path(std::string_view text, std::string_view architecture) {
    const auto bundle = path_from_utf8(text);
    return bundle / L"Contents" / std::filesystem::path(utf8_to_utf16(architecture)) / bundle.filename();
}
enum class PluginPathKind { Bundle, Dll, Error };
inline PluginPathKind inspected_plugin_kind(bool directory, const std::error_code& error) {
    if (error && error != std::errc::no_such_file_or_directory) return PluginPathKind::Error;
    return directory ? PluginPathKind::Bundle : PluginPathKind::Dll;
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
