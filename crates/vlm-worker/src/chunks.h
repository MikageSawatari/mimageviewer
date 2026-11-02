#pragma once
#include "mtmd.h"
#include <nlohmann/json.hpp>

// Text tokens plus image identity, token count and positions (never image bytes).
inline nlohmann::json describe_chunks(const mtmd_input_chunks * chunks) {
    using json = nlohmann::json;
    json out = json::array();
    for (size_t i = 0; i < mtmd_input_chunks_size(chunks); ++i) {
        auto c = mtmd_input_chunks_get(chunks, i);
        json row{{"type", mtmd_input_chunk_get_type(c)}, {"n_tokens", mtmd_input_chunk_get_n_tokens(c)},
            {"n_pos", mtmd_input_chunk_get_n_pos(c)}};
        if (mtmd_input_chunk_get_type(c) == MTMD_INPUT_CHUNK_TYPE_TEXT) {
            size_t n = 0;
            auto t = mtmd_input_chunk_get_tokens_text(c, &n);
            row["tokens"] = std::vector<llama_token>(t, t + n);
        } else {
            auto id = mtmd_input_chunk_get_id(c);
            row["id"] = id ? id : "";
        }
        out.push_back(row);
    }
    return out;
}
