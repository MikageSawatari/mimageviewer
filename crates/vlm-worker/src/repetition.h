#pragma once
#include "llama.h"
#include <memory>
#include <stdexcept>
#include <string_view>

// A lexical scope tracker, not a second JSON validator: the grammar owns validity.
// Only quote-free tokens wholly inside one value enter DRY's history. Therefore
// DRY can never penalize a closing quote, a mixed syntax token, a key or EOG.
class ValueRepetition {
    enum class String { Outside, Key, Value };
    String string_ = String::Outside;
    bool escaped_ = false;
    bool value_next_ = false;
    bool in_array_ = false; // The fixed caption schema has only one flat array.
    std::unique_ptr<llama_sampler, decltype(&llama_sampler_free)> dry_;
public:
    explicit ValueRepetition(const llama_vocab * vocab = nullptr)
        // No raw sequence breakers: b12032 does not access vocab in this case.
        // In particular, newline escapes must not break alternating-line loops.
        : dry_(llama_sampler_init_dry(vocab, 0.8f, 1.75f, 2, 256, nullptr, 0), llama_sampler_free) {
        if (!dry_) throw std::runtime_error("DRY sampler creation failed");
    }
    llama_sampler * sampler() const { return string_ == String::Value ? dry_.get() : nullptr; }

    void accept(llama_token token, std::string_view piece) {
        const bool content_only = string_ == String::Value && piece.find('"') == std::string_view::npos;
        for (char c : piece) {
            if (string_ != String::Outside) {
                if (escaped_) escaped_ = false;
                else if (c == '\\') escaped_ = true;
                else if (c == '"') string_ = String::Outside;
            } else {
                switch (c) {
                    case '"':
                        string_ = value_next_ ? String::Value : String::Key;
                        value_next_ = false;
                        break;
                    case ':': value_next_ = true; break;
                    case '[': in_array_ = true; value_next_ = true; break;
                    case ']': in_array_ = false; value_next_ = false; break;
                    case ',': value_next_ = in_array_; break;
                }
            }
        }
        if (content_only && string_ == String::Value) llama_sampler_accept(dry_.get(), token);
        else llama_sampler_reset(dry_.get());
    }
};
