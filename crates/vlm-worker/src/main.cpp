#include "devices.h"
#include "contracts.h"
#include "prompt.h"
#include "caption-grammar.h"
#include "chunks.h"
#include "greedy.h"
#include "engine_errors.h"
#include "image_input.h"
#include "exclusive_output.h"
#include "llama.h"
#include "mtmd-helper.h"
#include <algorithm>
#include <chrono>
#include <filesystem>
#include <cstdio>
#include <iostream>
#include <memory>
#include <map>
#include <cctype>
#include <stdexcept>

using json = nlohmann::json;
namespace fs = std::filesystem;
using Clock = std::chrono::steady_clock;
template<class T, void (*Free)(T *)> using Handle = std::unique_ptr<T, decltype(Free)>;
double seconds(Clock::time_point start) { return std::chrono::duration<double>(Clock::now() - start).count(); }
std::string piece(const llama_vocab * vocab, llama_token token) {
    std::string buf(256, '\0');
    int n = llama_token_to_piece(vocab, token, buf.data(), int(buf.size()), 0, true);
    if (n < 0) { buf.resize(-n); n = llama_token_to_piece(vocab, token, buf.data(), int(buf.size()), 0, true); }
    if (n < 0) throw std::runtime_error("Token conversion failed");
    buf.resize(n);
    return buf;
}
int run(const std::vector<std::string> & args) {
    if (args.size() == 1 && args[0] == "--list-devices") {
        std::cout << list_devices().dump(2) << '\n';
        return 0;
    }
    std::map<std::string, std::string> options;
    bool selftest = false;
    for (size_t i = 0; i < args.size(); ++i) {
        if (args[i] == "--selftest") { selftest = true; continue; }
        if (args[i] != "--model" && args[i] != "--mmproj" && args[i] != "--images" && args[i] != "--out"
            && args[i] != "--device" && args[i] != "--order") throw std::runtime_error("Unknown argument");
        if (i + 1 == args.size() || options.count(args[i])) throw std::runtime_error("Missing or duplicate option");
        auto key = args[i];
        options[key] = args[++i];
    }
    if (!selftest) throw std::runtime_error("Use --selftest --model GGUF --mmproj GGUF --images DIR --out JSONL [--device HIGH:LOW] [--order forward|reverse], or --list-devices");
    for (auto key : {"--model", "--mmproj", "--images", "--out"}) if (!options.count(key)) throw std::runtime_error("Required option missing");
    auto order = options.count("--order") ? options["--order"] : "forward";
    if (order != "forward" && order != "reverse") throw std::runtime_error("Order must be forward or reverse");
    auto out_path = fs::u8path(options["--out"]);
    std::vector<fs::path> images;
    for (auto & entry : fs::directory_iterator(fs::u8path(options["--images"]))) {
        auto ext = entry.path().extension().u8string();
        std::transform(ext.begin(), ext.end(), ext.begin(), [](unsigned char c) { return char(std::tolower(c)); });
        if (entry.is_regular_file() && ext == ".png") images.push_back(entry.path());
    }
    std::sort(images.begin(), images.end(), [](const auto & a, const auto & b) { return a.filename().u8string() < b.filename().u8string(); });
    if (images.empty()) throw std::runtime_error("No PNG inputs");
    if (order == "reverse") std::reverse(images.begin(), images.end());
    ExclusiveOutput out(out_path);
    static EngineErrors errors;
    auto log = [](ggml_log_level level, const char * message, void * user) {
        static_cast<EngineErrors *>(user)->observe(level, message);
        std::fputs(message, stderr);
    };
    // Only mtmd collapses resource failures into image failure return codes.
    mtmd_helper_log_set(log, &errors);
    auto selected = select_device(options["--device"]);
    auto start = Clock::now();
    auto mp = llama_model_default_params();
    ggml_backend_dev_t devices[] = {selected.device, nullptr};
    mp.devices = devices;
    mp.n_gpu_layers = 999;
    mp.split_mode = LLAMA_SPLIT_MODE_NONE;
    Handle<llama_model, llama_model_free> model(llama_model_load_from_file(options["--model"].c_str(), mp), llama_model_free);
    if (!model) throw std::runtime_error("Model load failed");
    auto vp = mtmd_context_params_default();
    vp.device = selected.device;
    vp.use_gpu = true;
    vp.n_threads = 24;
    vp.flash_attn_type = LLAMA_FLASH_ATTN_TYPE_AUTO;
    vp.print_timings = false;
    Handle<mtmd_context, mtmd_free> vision(mtmd_init_from_file(options["--mmproj"].c_str(), model.get(), vp), mtmd_free);
    if (!vision) throw std::runtime_error("mmproj load failed");
    errors.check_fatal();
    double load_s = seconds(start);
    auto vocab = llama_model_get_vocab(model.get());
    size_t failed = 0;
    for (size_t index = 0; index < images.size(); ++index) {
        auto t0 = Clock::now();
        json rec{{"label", "V1-0"}, {"prompt_version", "v0-3l"}, {"image", images[index].filename().u8string()},
            {"index", index}, {"order", order}, {"model_load_s", load_s}, {"device", selected.description}};
        run_image_attempt(rec, [&] {
            auto bytes = read_selftest_png(images[index]);
            mtmd_helper_init_opt init{};
            init.video_params = mtmd_helper_video_init_params_default();
            auto loaded = mtmd_helper_bitmap_init_from_buf(vision.get(), bytes.data(), bytes.size(), false, init);
            Handle<mtmd_bitmap, mtmd_bitmap_free> bitmap(loaded.bitmap, mtmd_bitmap_free);
            errors.check_fatal();
            if (!bitmap) throw ImageFailure("Image decode failed");
            check_image_dimensions(mtmd_bitmap_get_nx(bitmap.get()), mtmd_bitmap_get_ny(bitmap.get()));
            // Every image owns a fresh memory, sequence 0, position and sampler.
            auto phase = Clock::now();
            auto cp = llama_context_default_params();
            cp.n_ctx = 8192;
            cp.n_batch = 2048;
            cp.n_ubatch = 512;
            cp.n_seq_max = 1;
            cp.n_threads = 24;
            cp.n_threads_batch = 24;
            cp.flash_attn_type = LLAMA_FLASH_ATTN_TYPE_AUTO;
            Handle<llama_context, llama_free> context(llama_init_from_model(model.get(), cp), llama_free);
            if (!context) throw std::runtime_error("Context creation failed");
            errors.check_fatal();
            rec["timings"]["context_init_s"] = seconds(phase);
            phase = Clock::now();
            Handle<llama_sampler, llama_sampler_free> sampler(llama_sampler_chain_init(llama_sampler_chain_default_params()), llama_sampler_free);
            auto grammar = llama_sampler_init_grammar(vocab, kGrammar, "root");
            if (!grammar) throw std::runtime_error("Grammar creation failed");
            llama_sampler_chain_add(sampler.get(), grammar);
            llama_sampler_chain_add(sampler.get(), llama_sampler_init_greedy());
            rec["timings"]["sampler_init_s"] = seconds(phase);
            phase = Clock::now();
            auto prompt = fixed_prompt(mtmd_default_marker());
            mtmd_input_text text{prompt.data(), prompt.size(), true, true};
            Handle<mtmd_input_chunks, mtmd_input_chunks_free> chunks(mtmd_input_chunks_init(), mtmd_input_chunks_free);
            const mtmd_bitmap * bitmaps[] = {bitmap.get()};
            errors.check_tokenize(mtmd_tokenize(vision.get(), chunks.get(), &text, bitmaps, 1));
            rec["prompt_chunks"] = describe_chunks(chunks.get());
            size_t prompt_tokens = mtmd_helper_get_n_tokens(chunks.get());
            llama_pos position = 0;
            check_image_context_budget(prompt_tokens);
            errors.check_evaluation(mtmd_helper_eval_chunks(vision.get(), context.get(), chunks.get(), 0, 0, 2048, true, &position),
                "Image prompt evaluation failed");
            rec["timings"]["prompt_s"] = seconds(phase);
            phase = Clock::now();
            std::string content;
            int generated = 0;
            bool ended = false;
            Handle<llama_batch_ext, llama_batch_ext_free> batch(llama_batch_ext_init(context.get()), llama_batch_ext_free);
            if (!batch) throw std::runtime_error("Generation batch creation failed");
            for (int i = 0; i < 800; ++i) {
                auto token = greedy_with_grammar(sampler.get(), grammar, llama_get_logits_ith(context.get(), -1), llama_vocab_n_tokens(vocab));
                ++generated; // server usage includes the terminal token
                if (llama_vocab_is_eog(vocab, token)) { ended = true; break; }
                content += piece(vocab, token);
                llama_batch_ext_clear(batch.get());
                int slot = llama_batch_ext_add_token(batch.get(), 0, token);
                if (slot < 0 || !llama_batch_ext_set_pos(batch.get(), slot, &position)
                    || !llama_batch_ext_set_output_logits(batch.get(), slot, true)) throw std::runtime_error("Generation batch setup failed");
                ++position;
                int result = llama_process(context.get(), LLAMA_PROCESS_TYPE_DECODE, batch.get());
                errors.check_evaluation(result, "Image generation evaluation failed");
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
        });
        if (rec["status"] == "failed") ++failed;
        rec["wall_s"] = seconds(t0);
        out.write_line(rec.dump(-1, ' ', false, json::error_handler_t::replace));
        std::cout << "image=" << index + 1 << "/" << images.size() << " seconds=" << rec["wall_s"] << " status=" << rec["status"] << std::endl;
    }
    std::cout << "model_load_s=" << load_s << " failed=" << failed << std::endl;
    return failed ? 2 : 0;
}
int wmain(int argc, wchar_t ** argv) {
    try {
        std::vector<std::string> args;
        for (int i = 1; i < argc; ++i) args.push_back(fs::path(argv[i]).u8string());
        return run(args);
    } catch (const std::exception & error) {
        std::cerr << "worker: " << error.what() << '\n';
        return 1;
    }
}
