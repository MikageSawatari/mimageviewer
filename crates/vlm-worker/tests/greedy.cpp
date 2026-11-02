#include "greedy.h"
#include <iostream>
#include <memory>
#include <vector>

struct Mask { int rejected; int accepts = 0; int applies = 0; };
void apply(llama_sampler * sampler, llama_token_data_array * array) {
    auto & mask = *static_cast<Mask *>(sampler->ctx);
    ++mask.applies;
    for (size_t i = 0; i < array->size; ++i) if (array->data[i].id == mask.rejected)
        array->data[i].logit = -std::numeric_limits<float>::infinity();
}
void accept(llama_sampler * sampler, llama_token) { ++static_cast<Mask *>(sampler->ctx)->accepts; }
const char * name(const llama_sampler *) { return "test-mask"; }
int main() {
    int failures = 0;
    llama_sampler_i interface{};
    interface.name = name;
    interface.apply = apply;
    interface.accept = accept;
    // Includes legal maxima, forbidden maxima and tied logits.
    for (auto values : {std::vector<float>{1, 3, 2}, std::vector<float>{3, 3, 2}, std::vector<float>{3, 2, 3}}) {
        for (int rejected = -1; rejected < 3; ++rejected) {
            Mask mask{rejected};
            auto grammar = llama_sampler_init(&interface, &mask);
            auto chain = llama_sampler_chain_init(llama_sampler_chain_default_params());
            llama_sampler_chain_add(chain, grammar);
            llama_sampler_chain_add(chain, llama_sampler_init_greedy());
            auto actual = greedy_with_grammar(chain, grammar, values.data(), int(values.size()));
            std::vector<llama_token_data> candidates;
            for (llama_token i = 0; i < int(values.size()); ++i) candidates.push_back({i, values[i], 0});
            llama_token_data_array all{candidates.data(), candidates.size(), -1, false};
            llama_sampler_apply(chain, &all);
            if (actual != all.data[all.selected].id || mask.accepts != 1) ++failures;
            llama_sampler_free(chain);
        }
    }
    std::cout << "greedy equivalence failures=" << failures << '\n';
    return failures ? 1 : 0;
}
