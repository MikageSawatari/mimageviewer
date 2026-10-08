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
// Loader paths are observable by plugins through GetModuleFileNameW. Prefer
// normal absolute Win32 syntax below MAX_PATH (UTF-16 units, excluding NUL),
// only when Win32 normalization cannot change the inspected file's identity:
// plugins may append forward-slash asset names, which fail under \\?\ syntax
// (EffeTune missing-assets, v4.3.0). Host-only inspection keeps extended paths.
inline std::filesystem::path loader_path_from_utf8(std::string_view text) {
    const auto extended = path_from_utf8(text).native();
    std::wstring normal;
    const bool unc = extended.size() >= 8 && extended.rfind(LR"(\\?\)", 0) == 0 &&
        (extended[4] == L'U' || extended[4] == L'u') &&
        (extended[5] == L'N' || extended[5] == L'n') &&
        (extended[6] == L'C' || extended[6] == L'c') && extended[7] == L'\\';
    if (unc)
        normal = LR"(\\)" + extended.substr(8);
    else if (extended.size() >= 7 && extended.rfind(LR"(\\?\)", 0) == 0 &&
             ((extended[4] >= L'A' && extended[4] <= L'Z') ||
              (extended[4] >= L'a' && extended[4] <= L'z')) &&
             extended[5] == L':' && extended[6] == L'\\')
        normal = extended.substr(4);
    // Other device namespaces have no equivalent drive/UNC form.
    if (normal.empty() || normal.size() >= MAX_PATH) return std::filesystem::path(extended);

    // GetFullPathNameW alone leaves DOS device names (even with extensions) and
    // some intermediate trailing spaces intact. Exclude these before checking
    // the spelling round-trip, so CreateFile/LoadLibrary cannot reinterpret them.
    for (size_t start = unc ? 2 : 3; start < normal.size();) {
        const auto separator = normal.find(L'\\', start);
        const auto end = separator == std::wstring::npos ? normal.size() : separator;
        if (end > start && (normal[end - 1] == L'.' || normal[end - 1] == L' '))
            return std::filesystem::path(extended);
        const auto suffix = normal.find_first_of(L".:", start);
        auto stem = normal.substr(start, (std::min)(end, suffix) - start);
        while (!stem.empty() && stem.back() == L' ') stem.pop_back();
        for (auto& c : stem) if (c >= L'a' && c <= L'z') c -= L'a' - L'A';
        const bool numbered_device = stem.size() == 4 &&
            (stem.compare(0, 3, L"COM") == 0 || stem.compare(0, 3, L"LPT") == 0) &&
            ((stem[3] >= L'1' && stem[3] <= L'9') ||
             stem[3] == L'\u00b9' || stem[3] == L'\u00b2' || stem[3] == L'\u00b3');
        if (stem == L"CON" || stem == L"NUL" || stem == L"AUX" || stem == L"PRN" ||
            stem == L"CONIN$" || stem == L"CONOUT$" || numbered_device)
            return std::filesystem::path(extended);
        if (separator == std::wstring::npos) break;
        start = separator + 1;
    }

    wchar_t resolved[MAX_PATH];
    const DWORD length = GetFullPathNameW(normal.c_str(), MAX_PATH, resolved, nullptr);
    if (!length || length >= MAX_PATH) return std::filesystem::path(extended);
    const std::wstring absolute(resolved, length);
    const auto round_trip = unc ? LR"(\\?\UNC\)" + absolute.substr(2) : LR"(\\?\)" + absolute;
    // The UNC namespace marker is case-insensitive; compare all remaining units
    // exactly. In particular, do not silently resolve dot components here.
    if (round_trip != (unc ? LR"(\\?\UNC\)" + extended.substr(8) : extended))
        return std::filesystem::path(extended);
    return std::filesystem::path(normal);
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
