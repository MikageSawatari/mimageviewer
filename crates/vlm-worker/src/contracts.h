#pragma once
#include <nlohmann/json.hpp>
#include <string>

inline bool valid_utf8(const std::string & value) {
    for (size_t i = 0; i < value.size();) {
        auto lead = static_cast<unsigned char>(value[i++]);
        if (lead < 0x80) continue;
        unsigned scalar;
        unsigned minimum;
        size_t tail;
        if (lead >= 0xc2 && lead <= 0xdf) { scalar = lead & 0x1f; minimum = 0x80; tail = 1; }
        else if (lead >= 0xe0 && lead <= 0xef) { scalar = lead & 0x0f; minimum = 0x800; tail = 2; }
        else if (lead >= 0xf0 && lead <= 0xf4) { scalar = lead & 7; minimum = 0x10000; tail = 3; }
        else return false;
        if (tail > value.size() - i) return false;
        while (tail--) {
            auto c = static_cast<unsigned char>(value[i++]);
            if ((c & 0xc0) != 0x80) return false;
            scalar = (scalar << 6) | (c & 0x3f);
        }
        if (scalar < minimum || scalar > 0x10ffff || (scalar >= 0xd800 && scalar <= 0xdfff)) return false;
    }
    return true;
}

inline size_t unicode_length(const std::string & value) {
    size_t count = 0;
    for (unsigned char c : value) if ((c & 0xc0) != 0x80) ++count;
    return count;
}
inline nlohmann::json validate_output(const std::string & content, bool ended, int tokens) {
    using json = nlohmann::json;
    // The vendored lexer treats raw NUL as EOF, so validate the entire buffer first.
    bool encoding = content.find('\0') == std::string::npos && valid_utf8(content);
    auto obj = encoding ? json::parse(content, nullptr, false) : json(json::value_t::discarded);
    bool complete = encoding && !obj.is_discarded();
    bool types = complete && obj.is_object() && obj.contains("caption") && obj["caption"].is_string()
        && obj.contains("text") && obj["text"].is_string() && obj.contains("keywords") && obj["keywords"].is_array();
    if (types) for (auto & word : obj["keywords"]) types = types && word.is_string();
    bool limits = types && unicode_length(obj["caption"].get<std::string>()) <= 400
        && unicode_length(obj["text"].get<std::string>()) <= 400 && obj["keywords"].size() <= 20;
    if (limits) for (auto & word : obj["keywords"]) limits = limits && unicode_length(word.get<std::string>()) <= 32;
    return {{"encoding_valid", encoding}, {"ended_on_eog", ended}, {"json_complete", complete}, {"field_types", types}, {"limits", limits},
        {"token_limit", tokens <= 800}, {"ok", ended && complete && types && limits && tokens <= 800}};
}
