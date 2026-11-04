#pragma once
#include "llama.h"
#include <limits>
#include <vector>

// Apply penalties exactly once BEFORE either greedy path. Same result as
// penalties -> full grammar mask -> greedy; skip the full mask for legal maxima.
inline llama_token greedy_with_grammar(llama_sampler * chain, llama_sampler * grammar,
                                      const float * logits, int n_vocab, llama_sampler * penalties = nullptr) {
    std::vector<llama_token_data> candidates;
    if (penalties) {
        candidates.reserve(n_vocab);
        for (llama_token i = 0; i < n_vocab; ++i) candidates.push_back({i, logits[i], 0});
        llama_token_data_array all{candidates.data(), candidates.size(), -1, false};
        llama_sampler_apply(penalties, &all);
    }
    const auto logit = [&](llama_token i) { return penalties ? candidates[i].logit : logits[i]; };
    llama_token best = 0;
    for (llama_token i = 1; i < n_vocab; ++i) if (logit(i) > logit(best)) best = i;
    llama_token_data candidate{best, logit(best), 0};
    llama_token_data_array single{&candidate, 1, -1, false};
    llama_sampler_apply(grammar, &single);
    if (candidate.logit != -std::numeric_limits<float>::infinity()) {
        llama_sampler_accept(chain, best);
        return best;
    }
    if (!penalties) {
        candidates.reserve(n_vocab);
        for (llama_token i = 0; i < n_vocab; ++i) candidates.push_back({i, logits[i], 0});
    }
    llama_token_data_array all{candidates.data(), candidates.size(), -1, false};
    llama_sampler_apply(chain, &all);
    auto token = all.data[all.selected].id;
    llama_sampler_accept(chain, token);
    return token;
}
