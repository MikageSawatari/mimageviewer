#include "inference.h"
#include "contracts.h"
#include "prompt.h"
#include "caption-grammar.h"
#include "chunks.h"
#include "greedy.h"
#include "repetition.h"
#include "image_input.h"
#include <cstdio>
#include <stdexcept>

using json = nlohmann::json;
namespace {
void stderr_log(ggml_log_level, const char * message, void *) { if (message) std::fputs(message, stderr); }
std::string piece(const llama_vocab * vocab, llama_token token) {
    std::string buf(256, '\0');
    int n = llama_token_to_piece(vocab, token, buf.data(), int(buf.size()), 0, true);
    if (n < 0) { buf.resize(-n); n = llama_token_to_piece(vocab, token, buf.data(), int(buf.size()), 0, true); }
    if (n < 0) throw std::runtime_error("Token conversion failed");
    buf.resize(n);
    return buf;
}
}
void route_engine_logs_to_stderr() {
    llama_log_set(stderr_log, nullptr);
    ggml_log_set(stderr_log, nullptr);
    mtmd_helper_log_set(stderr_log, nullptr);
}
InferenceEngine::InferenceEngine(const std::string & model_path, const std::string & mmproj_path, const std::string & luid, int threads)
    : selected_(select_device(luid)), threads_(threads) {
    mtmd_helper_log_set([](ggml_log_level level, const char * message, void * user) {
        static_cast<EngineErrors *>(user)->observe(level, message);
        if (message) std::fputs(message, stderr);
    }, &errors_);
    auto start = Clock::now();
    auto mp = llama_model_default_params();
    ggml_backend_dev_t devices[] = {selected_.device, nullptr};
    mp.devices = devices;
    mp.n_gpu_layers = 999;
    mp.split_mode = LLAMA_SPLIT_MODE_NONE;
    model_.reset(llama_model_load_from_file(model_path.c_str(), mp));
    if (!model_) throw std::runtime_error("Model load failed");
    auto vp = mtmd_context_params_default();
    vp.device = selected_.device;
    vp.use_gpu = true;
    vp.n_threads = threads_;
    vp.flash_attn_type = LLAMA_FLASH_ATTN_TYPE_AUTO;
    vp.print_timings = false;
    vision_.reset(mtmd_init_from_file(mmproj_path.c_str(), model_.get(), vp));
    if (!vision_) throw std::runtime_error("mmproj load failed");
    errors_.check_fatal();
    load_s_ = seconds(start);
}
void InferenceEngine::caption_rgb(json & rec, uint32_t width, uint32_t height, const unsigned char * rgb, const char * bitmap_id) {
    check_image_dimensions(width, height);
    Handle<mtmd_bitmap, mtmd_bitmap_free> bitmap(mtmd_bitmap_init(width, height, rgb), mtmd_bitmap_free);
    if (!bitmap) throw std::runtime_error("RGB bitmap allocation failed");
    mtmd_bitmap_set_id(bitmap.get(), bitmap_id);
    auto vocab = llama_model_get_vocab(model_.get());
    // Every image owns a fresh memory, sequence 0, position and sampler.
    auto phase = Clock::now();
    auto cp = llama_context_default_params();
    cp.n_ctx = 8192;
    cp.n_batch = 2048;
    cp.n_ubatch = 512;
    cp.n_seq_max = 1;
    cp.n_threads = threads_;
    cp.n_threads_batch = threads_;
    cp.flash_attn_type = LLAMA_FLASH_ATTN_TYPE_AUTO;
    Handle<llama_context, llama_free> context(llama_init_from_model(model_.get(), cp), llama_free);
    if (!context) throw std::runtime_error("Context creation failed");
    errors_.check_fatal();
    rec["timings"]["context_init_s"] = seconds(phase);
    phase = Clock::now();
    Handle<llama_sampler, llama_sampler_free> sampler(llama_sampler_chain_init(llama_sampler_chain_default_params()), llama_sampler_free);
    auto grammar = llama_sampler_init_grammar(vocab, kGrammar, "root");
    if (!grammar) throw std::runtime_error("Grammar creation failed");
    llama_sampler_chain_add(sampler.get(), grammar);
    llama_sampler_chain_add(sampler.get(), llama_sampler_init_greedy());
    ValueRepetition repetition(vocab);
    rec["timings"]["sampler_init_s"] = seconds(phase);
    phase = Clock::now();
    auto prompt = fixed_prompt(mtmd_default_marker());
    mtmd_input_text text{prompt.data(), prompt.size(), true, true};
    Handle<mtmd_input_chunks, mtmd_input_chunks_free> chunks(mtmd_input_chunks_init(), mtmd_input_chunks_free);
    const mtmd_bitmap * bitmaps[] = {bitmap.get()};
    errors_.check_tokenize(mtmd_tokenize(vision_.get(), chunks.get(), &text, bitmaps, 1));
    rec["prompt_chunks"] = describe_chunks(chunks.get());
    size_t prompt_tokens = mtmd_helper_get_n_tokens(chunks.get());
    llama_pos position = 0;
    check_image_context_budget(prompt_tokens);
    errors_.check_evaluation(mtmd_helper_eval_chunks(vision_.get(), context.get(), chunks.get(), 0, 0, 2048, true, &position),
        "Image prompt evaluation failed");
    rec["timings"]["prompt_s"] = seconds(phase);
    phase = Clock::now();
    std::string content;
    int generated = 0;
    bool ended = false;
    Handle<llama_batch_ext, llama_batch_ext_free> batch(llama_batch_ext_init(context.get()), llama_batch_ext_free);
    if (!batch) throw std::runtime_error("Generation batch creation failed");
    for (int i = 0; i < 800; ++i) {
        auto token = greedy_with_grammar(sampler.get(), grammar, llama_get_logits_ith(context.get(), -1), llama_vocab_n_tokens(vocab), repetition.sampler());
        ++generated; // server usage includes the terminal token
        if (llama_vocab_is_eog(vocab, token)) { ended = true; break; }
        auto token_piece = piece(vocab, token);
        repetition.accept(token, token_piece);
        content += token_piece;
        llama_batch_ext_clear(batch.get());
        int slot = llama_batch_ext_add_token(batch.get(), 0, token);
        if (slot < 0 || !llama_batch_ext_set_pos(batch.get(), slot, &position)
            || !llama_batch_ext_set_output_logits(batch.get(), slot, true)) throw std::runtime_error("Generation batch setup failed");
        ++position;
        int result = llama_process(context.get(), LLAMA_PROCESS_TYPE_DECODE, batch.get());
        errors_.check_evaluation(result, "Image generation evaluation failed");
    }
    rec["timings"]["generation_s"] = seconds(phase);
    rec["content"] = content;
    rec["finish_reason"] = ended ? "stop" : "length";
    rec["generated_tokens"] = generated;
    rec["usage"] = {{"prompt_tokens", prompt_tokens}, {"completion_tokens", generated}, {"total_tokens", prompt_tokens + generated},
        {"prompt_tokens_details", {{"cached_tokens", 0}}}};
    auto validation = validate_output(content, ended, generated);
    rec["validation"] = validation;
    rec["json_ok"] = validation["json_complete"].get<bool>() && validation["field_types"].get<bool>();
    rec["status"] = validation["ok"].get<bool>() ? "ok" : "failed";
    if (validation["json_complete"].get<bool>()) {
        auto obj = json::parse(content);
        if (obj.is_object()) for (auto key : {"caption", "text", "keywords"}) if (obj.contains(key)) rec[key] = obj[key];
    }
}
void InferenceEngine::caption_png(json & rec, const std::filesystem::path & path) {
    auto bytes = read_selftest_png(path);
    mtmd_helper_init_opt init{};
    init.video_params = mtmd_helper_video_init_params_default();
    auto loaded = mtmd_helper_bitmap_init_from_buf(vision_.get(), bytes.data(), bytes.size(), false, init);
    Handle<mtmd_bitmap, mtmd_bitmap_free> bitmap(loaded.bitmap, mtmd_bitmap_free);
    errors_.check_fatal();
    if (!bitmap) throw ImageFailure("Image decode failed");
    caption_rgb(rec, mtmd_bitmap_get_nx(bitmap.get()), mtmd_bitmap_get_ny(bitmap.get()), mtmd_bitmap_get_data(bitmap.get()), mtmd_bitmap_get_id(bitmap.get()));
}
