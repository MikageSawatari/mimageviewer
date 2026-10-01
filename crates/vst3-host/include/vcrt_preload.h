#pragma once
#include <windows.h>
#include <array>
#include <cstdint>
#include <cstddef>

namespace miv {
enum class VcrtLocation { System32, Bundled, Error };
struct VcrtVersion {
    uint16_t major = 0, minor = 0, build = 0, revision = 0;
};
constexpr bool operator==(VcrtVersion a, VcrtVersion b) {
    return a.major == b.major && a.minor == b.minor && a.build == b.build && a.revision == b.revision;
}
constexpr bool operator>=(VcrtVersion a, VcrtVersion b) {
    if (a.major != b.major) return a.major > b.major;
    if (a.minor != b.minor) return a.minor > b.minor;
    if (a.build != b.build) return a.build > b.build;
    return a.revision >= b.revision;
}
struct VcrtFile {
    bool readable = false;
    VcrtVersion version{};
};
using VcrtSet = std::array<VcrtFile, 4>; // VCRUNTIME140, _1, MSVCP140, _1

// Choose one source for the whole set before loading anything. The bundled set
// must be complete and have one version. Use a complete readable system set only
// when its VCRUNTIME140 is at least that bundled version.
constexpr VcrtLocation choose_vcrt_set(const VcrtSet& system, const VcrtSet& bundled) {
    bool system_complete = true;
    for (size_t index = 0; index < bundled.size(); ++index) {
        if (!bundled[index].readable || !(bundled[index].version == bundled[0].version)) return VcrtLocation::Error;
        if (!system[index].readable) system_complete = false;
    }
    return system_complete && system[0].version >= bundled[0].version
        ? VcrtLocation::System32 : VcrtLocation::Bundled;
}
bool preload_vcrt();
} // namespace miv
