// プロトコルヘルパ用の実装ユニット (現状は header-only な定数のみだが、
// 将来 nlohmann/json を導入したらこちらに JSON シリアライズロジックを移す)。

#include "protocol.h"

namespace miv {
std::string gui_warning_json(uint64_t slot_id, uint64_t container_hwnd, const std::string& detail) {
    std::string escaped;
    for (unsigned char c : detail) {
        if (c == '"' || c == '\\') { escaped += '\\'; escaped += char(c); }
        else if (c < 0x20) {
            constexpr char hex[] = "0123456789abcdef";
            escaped += "\\u00"; escaped += hex[c >> 4]; escaped += hex[c & 15];
        } else escaped += char(c);
    }
    return "{\"event\":\"gui_warning\",\"slot_id\":" + std::to_string(slot_id) +
           ",\"container_hwnd\":" + std::to_string(container_hwnd) + ",\"detail\":\"" + escaped + "\"}";
}
std::string normal_rect_json_member(const std::optional<NormalScreenRect>& rect) {
    if (!rect) return {};
    return ",\"normal_rect\":{\"x\":" + std::to_string(rect->x) +
           ",\"y\":" + std::to_string(rect->y) +
           ",\"outer_width\":" + std::to_string(rect->outer_width) +
           ",\"outer_height\":" + std::to_string(rect->outer_height) + "}";
}
}  // namespace miv
