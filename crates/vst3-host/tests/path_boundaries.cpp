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
static_assert(miv::vcrt_location(true, ERROR_SUCCESS) == miv::VcrtLocation::System32);
static_assert(miv::vcrt_location(false, ERROR_FILE_NOT_FOUND) == miv::VcrtLocation::Bundled);
static_assert(miv::vcrt_location(false, ERROR_PATH_NOT_FOUND) == miv::VcrtLocation::Bundled);
static_assert(miv::vcrt_location(false, ERROR_ACCESS_DENIED) == miv::VcrtLocation::Error);

int main() {
    try {
        const auto acp = GetACP();
        const std::string text = u8R"(C:\Users\山田😀\音響調整\EffeTune Mixwright.vst3)";
        const std::wstring wide = LR"(C:\Users\山田😀\音響調整\EffeTune Mixwright.vst3)";
        require(miv::utf8_to_utf16(text) == wide);
        require(miv::utf16_to_utf8(wide) == text);
        const auto binary = miv::bundle_binary_path(text, "x86_64-win");
        require(binary.native() == wide + LR"(\Contents\x86_64-win\EffeTune Mixwright.vst3)");
        require(miv::path_to_utf8(binary).find(u8"山田😀") != std::string::npos);
        require(GetACP() == acp); // Plugin ANSI behavior remains unchanged.

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
        require(GetTempPathW(MAX_PATH, temporary) > 0);
        const auto dir = std::filesystem::path(temporary) /
            (L"miv-vst3-path-山田😀-" + std::to_wstring(GetCurrentProcessId()));
        const auto path = dir / L"設定.json";
        std::filesystem::create_directories(dir);
        try {
            std::ofstream output(miv::path_from_utf8(miv::path_to_utf8(path)), std::ios::binary);
            output << "opaque state";
            output.close();
            require(std::filesystem::file_size(path) == 12);
            const auto bundle = dir / L"音響調整.vst3";
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
