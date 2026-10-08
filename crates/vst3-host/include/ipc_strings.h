#pragma once
#include <cctype>
#include <cstdint>
#include <string>

namespace miv {
inline void json_space(const std::string& json, size_t& position) {
    while (position < json.size() && std::isspace(static_cast<unsigned char>(json[position]))) ++position;
}
inline bool json_hex4(const std::string& json, size_t& position, uint32_t& result) {
    result = 0;
    for (int digit = 0; digit < 4; ++digit) {
        if (position == json.size()) return false;
        const char c = json[position++];
        const int value = c >= '0' && c <= '9' ? c - '0' :
            c >= 'a' && c <= 'f' ? c - 'a' + 10 : c >= 'A' && c <= 'F' ? c - 'A' + 10 : -1;
        if (value < 0) return false;
        result = result * 16 + static_cast<uint32_t>(value);
    }
    return true;
}
inline void json_utf8(std::string& out, uint32_t codepoint) {
    if (codepoint < 0x80) out += static_cast<char>(codepoint);
    else if (codepoint < 0x800) {
        out += static_cast<char>(0xc0 | (codepoint >> 6));
        out += static_cast<char>(0x80 | (codepoint & 0x3f));
    } else if (codepoint < 0x10000) {
        out += static_cast<char>(0xe0 | (codepoint >> 12));
        out += static_cast<char>(0x80 | ((codepoint >> 6) & 0x3f));
        out += static_cast<char>(0x80 | (codepoint & 0x3f));
    } else {
        out += static_cast<char>(0xf0 | (codepoint >> 18));
        out += static_cast<char>(0x80 | ((codepoint >> 12) & 0x3f));
        out += static_cast<char>(0x80 | ((codepoint >> 6) & 0x3f));
        out += static_cast<char>(0x80 | (codepoint & 0x3f));
    }
}
inline bool json_string(const std::string& json, size_t& position, std::string& out) {
    out.clear();
    if (position == json.size() || json[position++] != '"') return false;
    while (position < json.size()) {
        const char c = json[position++];
        if (c == '"') return true;
        if (static_cast<unsigned char>(c) < 0x20) return false;
        if (c != '\\') { out += c; continue; }
        if (position == json.size()) return false;
        switch (json[position++]) {
        case '"': out += '"'; break;
        case '\\': out += '\\'; break;
        case '/': out += '/'; break;
        case 'b': out += '\b'; break;
        case 'f': out += '\f'; break;
        case 'n': out += '\n'; break;
        case 'r': out += '\r'; break;
        case 't': out += '\t'; break;
        case 'u': {
            uint32_t codepoint;
            if (!json_hex4(json, position, codepoint)) return false;
            if (codepoint >= 0xd800 && codepoint <= 0xdbff) {
                if (json.size() - position < 2 || json[position++] != '\\' || json[position++] != 'u') return false;
                uint32_t low;
                if (!json_hex4(json, position, low) || low < 0xdc00 || low > 0xdfff) return false;
                codepoint = 0x10000 + ((codepoint - 0xd800) << 10) + low - 0xdc00;
            } else if (codepoint >= 0xdc00 && codepoint <= 0xdfff) return false;
            json_utf8(out, codepoint);
            break;
        }
        default: return false;
        }
    }
    return false;
}

// Decode top-level control-object fields. Escaped path separators, quotes and
// Unicode must reach both probe and load as the same UTF-8 string Rust sent.
inline std::string extract_json_string_field(const std::string& json, const std::string& key) {
    size_t position = 0;
    json_space(json, position);
    if (position == json.size() || json[position++] != '{') return {};
    while (position < json.size()) {
        json_space(json, position);
        if (position == json.size() || json[position] == '}') return {};
        std::string name;
        if (!json_string(json, position, name)) return {};
        json_space(json, position);
        if (position == json.size() || json[position++] != ':') return {};
        json_space(json, position);
        if (position == json.size()) return {};
        if (json[position] == '"') {
            std::string value;
            if (!json_string(json, position, value)) return {};
            if (name == key) return value;
        } else {
            if (name == key) return {};
            int nesting = 0;
            while (position < json.size()) {
                const char c = json[position];
                if (c == '"') {
                    std::string ignored;
                    if (!json_string(json, position, ignored)) return {};
                    continue;
                }
                if (nesting == 0 && (c == ',' || c == '}')) break;
                if (c == '[' || c == '{') ++nesting;
                if (c == ']' || c == '}') --nesting;
                if (nesting < 0) return {};
                ++position;
            }
        }
        json_space(json, position);
        if (position == json.size() || json[position++] != ',') return {};
    }
    return {};
}
} // namespace miv
