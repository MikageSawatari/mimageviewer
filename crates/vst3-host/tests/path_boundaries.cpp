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
    for (size_t index = 0; index < bundled_set.size(); ++index) {
        auto system = crt_set({14, 51, 0, 0});
        system[index].readable = false;
        if (miv::choose_vcrt_set(system, bundled_set) != miv::VcrtLocation::Bundled) return false;
    }
    return true;
}
constexpr bool mixed_system_versions_select_one_set() {
    for (size_t index = 0; index < bundled_set.size(); ++index) {
        auto system = crt_set({14, 51, 0, 0});
        system[index].version = {14, 29, 0, 0};
        if (miv::choose_vcrt_set(system, bundled_set) != miv::VcrtLocation::Bundled) return false;
        system[index].version = bundled_version;
        if (miv::choose_vcrt_set(system, bundled_set) != miv::VcrtLocation::System32) return false;
        auto one_newer = crt_set({14, 29, 0, 0});
        one_newer[index].version = {14, 51, 0, 0};
        if (miv::choose_vcrt_set(one_newer, bundled_set) != miv::VcrtLocation::Bundled) return false;
    }
    return true;
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
static_assert(mixed_system_versions_select_one_set());
static_assert(invalid_bundled_set_is_rejected());

// Ask from inside a freshly loaded DLL, as a plugin deriving its asset root does.
static void check_module_filename(const std::filesystem::path& dll) {
    const auto module = LoadLibraryW(dll.c_str());
    require(module != nullptr);
    try {
        using Probe = DWORD (*)(wchar_t*, DWORD);
        const auto probe = reinterpret_cast<Probe>(GetProcAddress(module, "fixture_module_filename"));
        require(probe != nullptr);
        wchar_t filename[32768];
        const auto length = probe(filename, 32768);
        require(length > 0 && length < 32768);
        require(std::wstring(filename, length) == dll.native());
        std::cout << "GetModuleFileNameW preserves " <<
            (dll.native().rfind(LR"(\\?\)", 0) == 0 ? "extended" : "normal") << " loader syntax\n";
    } catch (...) {
        FreeLibrary(module);
        throw;
    }
    require(FreeLibrary(module) != 0);
}

int wmain(int argc, wchar_t** argv) {
    try {
        require(argc == 2); // CTest supplies the inert fixture's absolute path.
        const auto acp = GetACP();
        const std::string text = u8R"(C:\Users\山田😀\音響調整\EffeTune Mixwright.vst3)";
        const std::wstring wide = LR"(C:\Users\山田😀\音響調整\EffeTune Mixwright.vst3)";
        require(miv::utf8_to_utf16(text) == wide);
        require(miv::utf16_to_utf8(wide) == text);
        const auto binary = miv::bundle_binary_path(text, "x86_64-win");
        require(binary.native() == LR"(\\?\)" + wide + LR"(\Contents\x86_64-win\EffeTune Mixwright.vst3)");
        require(miv::loader_path_from_utf8(miv::path_to_utf8(binary)).native() ==
            wide + LR"(\Contents\x86_64-win\EffeTune Mixwright.vst3)");
        require(miv::loader_path_from_utf8(text).native() == wide);
        require(miv::loader_path_from_utf8(u8R"(C:/Users/山田😀/音響調整/EffeTune Mixwright.vst3)").native() == wide);
        require(miv::loader_path_from_utf8(u8R"(//?/C:/Users/山田/音響調整.vst3)").native() == LR"(C:\Users\山田\音響調整.vst3)");
        require(miv::loader_path_from_utf8(u8R"(//server/share/山田😀/音響調整.vst3)").native() == LR"(\\server\share\山田😀\音響調整.vst3)");
        require(miv::loader_path_from_utf8(u8R"(\\?\UNC\server\share\山田😀)").native() == LR"(\\server\share\山田😀)");
        require(miv::loader_path_from_utf8(u8R"(\\?\unc\server\share\山田😀)").native() == LR"(\\server\share\山田😀)");
        require(miv::loader_path_from_utf8(u8R"(\\?\UnC\server\share\山田😀)").native() == LR"(\\server\share\山田😀)");
        // Only a spelling-preserving normal candidate may replace the inspected
        // extended path. Device names need explicit checks: GetFullPathNameW can
        // leave them unchanged even though file APIs resolve them as devices.
        for (const auto* unsafe : {
                LR"(\\?\C:\plugins.\Effect.vst3)", LR"(\\?\C:\plugins \Effect.vst3)",
                LR"(\\?\C:\plugins\Effect.vst3.)", LR"(\\?\C:\plugins\Effect.vst3 )",
                LR"(\\?\C:\plugins\..\Effect.vst3)", LR"(\\?\C:\plugins\.\Effect.vst3)",
                LR"(\\?\UNC\server\share\plugins.\Effect.vst3)",
                LR"(\\?\unc\server\share\plugins \Effect.vst3)"}) {
            require(miv::loader_path_from_utf8(miv::utf16_to_utf8(unsafe)).native() == unsafe);
        }
        for (const auto* device : {L"CON", L"NUL", L"PRN", L"AUX", L"COM1", L"COM9",
                L"LPT1", L"LPT9", L"con", L"NuL", L"cOm1", L"CONIN$", L"CONOUT$",
                L"COM\u00b9", L"LPT\u00b2", L"COM\u00b3", L"NUL .vst3"}) {
            for (const auto* prefix : {LR"(\\?\C:\plugins\)", LR"(\\?\UnC\server\share\)"}) {
                const std::wstring base = std::wstring(prefix) + device;
                for (const auto* suffix : {L"", L".vst3", LR"(\Effect.vst3)"}) {
                    const auto input = base + suffix;
                    require(miv::loader_path_from_utf8(miv::utf16_to_utf8(input)).native() == input);
                }
            }
        }
        const auto ordinary = miv::loader_path_from_utf8(u8R"(\\?\C:\plugins\音響😀\Effect.vst3)");
        require(ordinary.native() == LR"(C:\plugins\音響😀\Effect.vst3)");
        require(miv::path_from_utf8(miv::path_to_utf8(ordinary)).native() ==
            LR"(\\?\C:\plugins\音響😀\Effect.vst3)");
        for (const auto* non_device : {L"CONCERT", L"NULify", L"COM0", L"COM10", L"LPT0", L"LPT10"}) {
            const auto normal = std::wstring(LR"(C:\plugins\)") + non_device + L".vst3";
            require(miv::loader_path_from_utf8(miv::utf16_to_utf8(LR"(\\?\)" + normal)).native() == normal);
        }
        require(miv::loader_path_from_utf8("./loader-fixture.dll").is_absolute());
        require(miv::path_to_utf8(binary).find(u8"山田😀") != std::string::npos);
        require(miv::path_from_utf8(u8R"(//?/C:/Users/山田/音響調整.vst3)").native() == LR"(\\?\C:\Users\山田\音響調整.vst3)");
        require(miv::path_from_utf8(u8R"(\\server\share\山田)").native() == LR"(\\?\UNC\server\share\山田)");
        const auto long_local = miv::path_from_utf8("C:/" + std::string(280, 'a') + u8"/音響調整.vst3");
        require(long_local.native().rfind(LR"(\\?\)", 0) == 0);
        require(long_local.native().find(L'/') == std::wstring::npos);
        require(miv::loader_path_from_utf8(miv::path_to_utf8(long_local)) == long_local);
        const std::string boundary = "C:/" + std::string(MAX_PATH - 5, 'a') + u8"😀";
        require(miv::utf8_to_utf16(boundary).size() == MAX_PATH);
        require(miv::loader_path_from_utf8(boundary).native().rfind(LR"(\\?\)", 0) == 0);
        require(miv::loader_path_from_utf8("C:/" + std::string(MAX_PATH - 4, 'a')).native().size() == MAX_PATH - 1);
        require(miv::loader_path_from_utf8("//server/share/" + std::string(280, 'a')).native().rfind(LR"(\\?\UNC\)", 0) == 0);
        const auto long_mixed_unc = std::string(R"(\\?\UnC\server\share\)") + std::string(280, 'a');
        require(miv::loader_path_from_utf8(long_mixed_unc).native() == miv::utf8_to_utf16(long_mixed_unc));
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
        invalid_rejected = false;
        try { miv::loader_path_from_utf8(std::string("\xff", 1)); }
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
            const auto dll = dir / L"loader-fixture.dll";
            std::filesystem::copy_file(argv[1], dll);
            check_module_filename(dll); // Reproduces the pre-fix module path.
            const auto normal_dll = miv::loader_path_from_utf8(miv::path_to_utf8(dll));
            require(normal_dll.native().size() < MAX_PATH);
            check_module_filename(normal_dll);
            const auto webview = dir / L"webview";
            std::filesystem::create_directories(webview / L"css");
            std::ofstream(webview / L"css" / L"effetune.css") << "fixture asset";
            const auto normal_asset = miv::loader_path_from_utf8(miv::path_to_utf8(webview)).native() + L"/css/effetune.css";
            const auto extended_asset = webview.native() + L"/css/effetune.css";
            require(GetFileAttributesW(normal_asset.c_str()) != INVALID_FILE_ATTRIBUTES);
            require(GetFileAttributesW(extended_asset.c_str()) == INVALID_FILE_ATTRIBUTES);
            std::cout << "Forward-slash asset tail: normal succeeds, extended fails (error=" <<
                GetLastError() << ")\n";
            std::ofstream output(miv::path_from_utf8(miv::path_to_utf8(path)), std::ios::binary);
            output << "opaque state";
            output.close();
            require(std::filesystem::file_size(path) == 12);
            const auto bundle = dir / std::wstring(220, L'x') / L"音響調整.vst3";
            const auto plugin = miv::bundle_binary_path(miv::path_to_utf8(bundle), "x86_64-win");
            std::filesystem::create_directories(plugin.parent_path());
            const auto long_dll = plugin.parent_path() / L"loader-fixture.dll";
            std::filesystem::copy_file(argv[1], long_dll);
            const auto long_loader = miv::loader_path_from_utf8(miv::path_to_utf8(long_dll));
            require(long_loader.native().size() > MAX_PATH + 4);
            require(long_loader == long_dll);
            check_module_filename(long_loader);
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
