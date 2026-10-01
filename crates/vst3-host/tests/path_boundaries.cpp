#include "utf8_paths.h"
#include "ipc_strings.h"
#include "vcrt_preload.h"
#include <fstream>
#include <iostream>

// A standalone helper test, never the real host and never a real plugin.
// Do not use assert: Release compiles with NDEBUG.
static void require(bool condition) {
    if (!condition) throw std::runtime_error("path-boundary regression");
}
constexpr miv::VcrtSet crt_set(miv::VcrtVersion version) {
    return {{{true, version}, {true, version}, {true, version}, {true, version}}};
}
constexpr miv::VcrtVersion bundled_version{14, 50, 35719, 0};
constexpr auto bundled_set = crt_set(bundled_version);
static_assert(miv::choose_vcrt_set(crt_set({14, 51, 0, 0}), bundled_set) == miv::VcrtLocation::System32);
static_assert(miv::choose_vcrt_set(crt_set({14, 50, 35719, 0}), bundled_set) == miv::VcrtLocation::System32);
static_assert(miv::choose_vcrt_set(crt_set({14, 29, 0, 0}), bundled_set) == miv::VcrtLocation::Bundled);
constexpr bool incomplete_system_uses_one_bundled_set() {
    auto system = crt_set({14, 51, 0, 0});
    system[2] = {}; // A missing companion never creates a mixed-source set.
    return miv::choose_vcrt_set(system, bundled_set) == miv::VcrtLocation::Bundled;
}
constexpr bool unreadable_system_uses_one_bundled_set() {
    auto system = crt_set({14, 51, 0, 0});
    system[0].readable = false;
    return miv::choose_vcrt_set(system, bundled_set) == miv::VcrtLocation::Bundled;
}
constexpr bool invalid_bundled_set_is_rejected() {
    auto unreadable = bundled_set;
    unreadable[1].readable = false;
    auto inconsistent = bundled_set;
    inconsistent[3].version.build -= 1;
    return miv::choose_vcrt_set(crt_set({14, 51, 0, 0}), unreadable) == miv::VcrtLocation::Error &&
        miv::choose_vcrt_set(crt_set({14, 51, 0, 0}), inconsistent) == miv::VcrtLocation::Error;
}
static_assert(incomplete_system_uses_one_bundled_set());
static_assert(unreadable_system_uses_one_bundled_set());
static_assert(invalid_bundled_set_is_rejected());

int main() {
    try {
        const auto acp = GetACP();
        const std::string text = u8R"(C:\Users\山田😀\音響調整\EffeTune Mixwright.vst3)";
        const std::wstring wide = LR"(C:\Users\山田😀\音響調整\EffeTune Mixwright.vst3)";
        require(miv::utf8_to_utf16(text) == wide);
        require(miv::utf16_to_utf8(wide) == text);
        const auto binary = miv::bundle_binary_path(text, "x86_64-win");
        require(binary.native() == LR"(\\?\)" + wide + LR"(\Contents\x86_64-win\EffeTune Mixwright.vst3)");
        require(miv::path_to_utf8(binary).find(u8"山田😀") != std::string::npos);
        require(miv::path_from_utf8(u8R"(//?/C:/Users/山田/音響調整.vst3)").native() == LR"(\\?\C:\Users\山田\音響調整.vst3)");
        require(miv::path_from_utf8(u8R"(\\server\share\山田)").native() == LR"(\\?\UNC\server\share\山田)");
        const auto long_local = miv::path_from_utf8("C:/" + std::string(280, 'a') + u8"/音響調整.vst3");
        require(long_local.native().rfind(LR"(\\?\)", 0) == 0);
        require(long_local.native().find(L'/') == std::wstring::npos);
        require(GetACP() == acp); // Plugin ANSI behavior remains unchanged.
        require(miv::inspected_plugin_kind(true, {}) == miv::PluginPathKind::Bundle);
        require(miv::inspected_plugin_kind(false, {}) == miv::PluginPathKind::Dll);
        require(miv::inspected_plugin_kind(false, std::make_error_code(std::errc::no_such_file_or_directory)) == miv::PluginPathKind::Dll);
        require(miv::inspected_plugin_kind(false, std::make_error_code(std::errc::permission_denied)) == miv::PluginPathKind::Error);
        require(miv::inspected_plugin_kind(false, std::make_error_code(std::errc::filename_too_long)) == miv::PluginPathKind::Error);

        const auto escaped = R"({"count":2,"nested":{"plugin_path":"wrong"},"plugin_path":"C:\\Users\\山田😀\\EffeTune Mixwright.vst3","state":"AA=="})";
        require(miv::extract_json_string_field(escaped, "plugin_path") == u8R"(C:\Users\山田😀\EffeTune Mixwright.vst3)");
        require(miv::extract_json_string_field(escaped, "state") == "AA==");
        require(miv::extract_json_string_field(R"({"plugin_path":"C:\\Users\\山田\\a\"b.vst3"})", "plugin_path") == u8R"(C:\Users\山田\a"b.vst3)");
        require(miv::extract_json_string_field(R"({"plugin_path":"\u5c71\u7530\ud83d\ude00"})", "plugin_path") == u8"山田😀");
        require(miv::extract_json_string_field(R"({"plugin_path":"\ud800"})", "plugin_path").empty());
        bool invalid_rejected = false;
        try { miv::path_from_utf8(std::string("\xff", 1)); }
        catch (const std::exception&) { invalid_rejected = true; }
        require(invalid_rejected);
        require(miv::display_title_from_utf8(std::string("\xff", 1)) == L"VST3 Plugin");
        require(miv::display_title_from_utf8({}) == L"VST3 Plugin");
        require(miv::display_title_from_utf8(u8"音響調整😀") == L"音響調整😀");

        // Filesystem constructors and streams use a native wide path, including
        // non-BMP text and a bundle tree under a Japanese profile-like folder.
        wchar_t temporary[MAX_PATH];
        const auto temporary_length = GetTempPathW(MAX_PATH, temporary);
        require(temporary_length > 0 && temporary_length < MAX_PATH);
        const auto dir = miv::native_windows_path(std::wstring(temporary) +
            L"miv-vst3-path-山田😀-" + std::to_wstring(GetCurrentProcessId()));
        const auto path = dir / L"設定.json";
        std::filesystem::create_directories(dir);
        try {
            std::ofstream output(miv::path_from_utf8(miv::path_to_utf8(path)), std::ios::binary);
            output << "opaque state";
            output.close();
            require(std::filesystem::file_size(path) == 12);
            const auto bundle = dir / std::wstring(220, L'x') / L"音響調整.vst3";
            const auto plugin = miv::bundle_binary_path(miv::path_to_utf8(bundle), "x86_64-win");
            std::filesystem::create_directories(plugin.parent_path());
            std::ofstream(plugin, std::ios::binary) << "dummy, not PE";
            const auto file = CreateFileW(plugin.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, 0, nullptr);
            require(file != INVALID_HANDLE_VALUE);
            CloseHandle(file);
        } catch (...) {
            std::filesystem::remove_all(dir);
            throw;
        }
        std::filesystem::remove_all(dir);
        return 0;
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
