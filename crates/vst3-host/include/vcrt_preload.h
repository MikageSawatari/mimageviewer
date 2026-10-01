#pragma once
#include <windows.h>

namespace miv {
enum class VcrtLocation { System32, Bundled, Error };
// Fall back only when the DLL itself is missing. A present but unloadable system
// CRT (permissions, invalid image, missing export/dependency) must not be hidden.
constexpr VcrtLocation vcrt_location(bool system_file_present, DWORD error) {
    return system_file_present ? VcrtLocation::System32 :
        (error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND)
            ? VcrtLocation::Bundled : VcrtLocation::Error;
}
bool preload_vcrt();
} // namespace miv
