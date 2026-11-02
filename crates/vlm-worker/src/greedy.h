#pragma once
#include "llama.h"
#include <limits>
#include <vector>

// Same result as masking every logit first; avoid that cost for legal maxima.
inline llama_token greedy_with_grammar(llama_sampler * chain, llama_sampler * grammar,
                                      const float * logits, int n_vocab) {
    llama_token best = 0;
    for (llama_token i = 1; i < n_vocab; ++i) if (logits[i] > logits[best]) best = i;
    llama_token_data candidate{best, logits[best], 0};
    llama_token_data_array single{&candidate, 1, -1, false};
    llama_sampler_apply(grammar, &single);
    if (candidate.logit != -std::numeric_limits<float>::infinity()) {
        llama_sampler_accept(chain, best);
        return best;
    }
    std::vector<llama_token_data> candidates;
    candidates.reserve(n_vocab);
    for (llama_token i = 0; i < n_vocab; ++i) candidates.push_back({i, logits[i], 0});
    llama_token_data_array all{candidates.data(), candidates.size(), -1, false};
    llama_sampler_apply(chain, &all);
    auto token = all.data[all.selected].id;
    llama_sampler_accept(chain, token);
    return token;
}
