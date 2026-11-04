#include "greedy.h"
#include "repetition.h"
#include "contracts.h"
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
std::vector<float> adjusted(ValueRepetition & repetition, const std::vector<float> & logits) {
    std::vector<llama_token_data> candidates;
    for (llama_token i = 0; i < int(logits.size()); ++i) candidates.push_back({i, logits[i], 0});
    llama_token_data_array all{candidates.data(), candidates.size(), -1, false};
    if (auto sampler = repetition.sampler()) llama_sampler_apply(sampler, &all);
    std::vector<float> result;
    for (const auto & candidate : candidates) result.push_back(candidate.logit);
    return result;
}
int main() {
    int failures = 0;
    auto check = [&](bool ok, const char * message) {
        if (!ok) { ++failures; std::cerr << message << '\n'; }
    };
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
    // The full-mask reference must see the same once-penalized logits. Exercise
    // both paths, changed maxima, ties, and a rejected maximum after DRY.
    for (auto values : {std::vector<float>{2, 1.5f, 1}, std::vector<float>{2, 1, 1}, std::vector<float>{3, 1, 2}}) {
        for (int rejected = -1; rejected < 3; ++rejected) {
            ValueRepetition repetition;
            repetition.accept(2, "{\"caption\":\"");
            for (int i = 0; i < 3; ++i) repetition.accept(0, "x");
            Mask mask{rejected};
            auto grammar = llama_sampler_init(&interface, &mask);
            auto chain = llama_sampler_chain_init(llama_sampler_chain_default_params());
            llama_sampler_chain_add(chain, grammar);
            llama_sampler_chain_add(chain, llama_sampler_init_greedy());
            auto penalized = adjusted(repetition, values);
            auto actual = greedy_with_grammar(chain, grammar, values.data(), int(values.size()), repetition.sampler());
            const int applies = mask.applies;
            int best = 0;
            for (int i = 1; i < int(penalized.size()); ++i) if (penalized[i] > penalized[best]) best = i;
            check(mask.accepts == 1 && applies == (rejected == best ? 2 : 1), "penalized fast/fallback path accounting");
            check(actual == greedy_with_grammar(chain, grammar, penalized.data(), int(values.size())), "DRY greedy equivalence");
            llama_sampler_free(chain);
        }
    }
    {
        ValueRepetition repetition;
        repetition.accept(4, "{\"caption\":\"");
        auto initial = adjusted(repetition, {3, 3, 3, 3, 3});
        for (int i = 0; i < 3; ++i) repetition.accept(0, "x");
        auto repeated = adjusted(repetition, initial);
        check(repeated[0] < initial[0] && repeated[1] == initial[1], "single-token loop penalty");
        repetition.accept(4, "\",\"text\":\"");
        check(adjusted(repetition, initial) == initial, "mixed boundary token resets history");
        // The same token IDs are allowed again in a different field/image.
        ValueRepetition fresh;
        fresh.accept(4, "{\"caption\":\"");
        check(adjusted(fresh, initial) == initial, "per-image history");
    }
    {
        ValueRepetition repetition;
        repetition.accept(6, "{\"caption\":\"");
        for (int i = 0; i < 2; ++i) {
            repetition.accept(0, "alpha"); repetition.accept(1, "\\n");
            repetition.accept(2, "beta"); repetition.accept(1, "\\n");
        }
        auto result = adjusted(repetition, {5, 5, 5, 5, 5, 5, 5});
        check(result[0] < 5, "alternating lines including escaped newline");
        check(result[3] == 5 && result[6] == 5, "unseen token and quote-bearing boundary protected");
        // Quotes escaped across token boundaries do not become JSON delimiters.
        repetition.accept(3, "\\"); repetition.accept(4, "\"");
        check(repetition.sampler() != nullptr, "split escaped quote remains in value");
        for (int i = 0; i < 3; ++i) repetition.accept(0, "x");
        repetition.accept(5, "\\\\"); repetition.accept(4, "\"");
        check(repetition.sampler() == nullptr, "even backslashes allow closing quote");
        repetition.accept(6, ",\"text");
        repetition.accept(0, "x");
        check(repetition.sampler() == nullptr, "key never activates DRY");
    }
    {
        // Synthetic generation keeps preferring a loop until DRY lets the
        // closing quote win. All three fields use the same token IDs, so this
        // also catches leakage across fields/array elements and mixed tokens.
        ValueRepetition repetition;
        Mask mask{-1};
        auto grammar = llama_sampler_init(&interface, &mask);
        auto chain = llama_sampler_chain_init(llama_sampler_chain_default_params());
        llama_sampler_chain_add(chain, grammar);
        llama_sampler_chain_add(chain, llama_sampler_init_greedy());
        std::string output;
        int generated = 0;
        auto emit = [&](llama_token token, const std::string & text) {
            repetition.accept(token, text); output += text; ++generated;
        };
        auto value = [&] {
            for (int i = 0; i < 20; ++i) {
                float logits[] = {2, 1.5f}; // x, closing quote
                auto token = greedy_with_grammar(chain, grammar, logits, 2, repetition.sampler());
                emit(token, token == 0 ? "x" : "\"");
                if (token == 1) return;
            }
            check(false, "synthetic loop did not terminate");
        };
        emit(2, "{\"caption\":\""); value();
        emit(3, ",\"text\":\""); value();
        emit(4, ",\"keywords\":[\""); value();
        emit(5, ",\""); value(); emit(6, "]}");
        check(validate_output(output, true, generated)["ok"].get<bool>(), "generated JSON validates all contracts");
        auto parsed = nlohmann::json::parse(output);
        check(parsed["caption"] == "xxx" && parsed["text"] == "xxx"
            && parsed["keywords"] == nlohmann::json::array({"xxx", "xxx"}), "field and item histories are independent");
        llama_sampler_free(chain);
    }
    std::cout << "greedy equivalence failures=" << failures << '\n';
    return failures ? 1 : 0;
}
