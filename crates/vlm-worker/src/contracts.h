#pragma once
#include <nlohmann/json.hpp>
#include <string>

inline size_t unicode_length(const std::string & value) {
    size_t count = 0;
    for (unsigned char c : value) if ((c & 0xc0) != 0x80) ++count;
    return count;
}
inline nlohmann::json validate_output(const std::string & content, bool ended, int tokens) {
    using json = nlohmann::json;
    auto obj = json::parse(content, nullptr, false);
    bool complete = !obj.is_discarded();
    bool types = complete && obj.is_object() && obj.contains("caption") && obj["caption"].is_string()
        && obj.contains("text") && obj["text"].is_string() && obj.contains("keywords") && obj["keywords"].is_array();
    if (types) for (auto & word : obj["keywords"]) types = types && word.is_string();
    bool limits = types && unicode_length(obj["caption"].get<std::string>()) <= 400
        && unicode_length(obj["text"].get<std::string>()) <= 400 && obj["keywords"].size() <= 20;
    if (limits) for (auto & word : obj["keywords"]) limits = limits && unicode_length(word.get<std::string>()) <= 32;
    return {{"ended_on_eog", ended}, {"json_complete", complete}, {"field_types", types}, {"limits", limits},
        {"token_limit", tokens <= 800}, {"ok", ended && complete && types && limits && tokens <= 800}};
}
