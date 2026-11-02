// Offline development tool only. Never shipped with the worker.
#include "prompt.h"
#include "chunks.h"
#include "chat.h"
#include "json-schema-to-grammar.h"
#include "mtmd-helper.h"
#include <filesystem>
#include <fstream>
#include <iostream>
#include <algorithm>

namespace fs = std::filesystem;
using json = nlohmann::json;
int wmain(int argc, wchar_t ** wide_argv) {
    try {
        std::vector<std::string> argv;
        for (int i = 0; i < argc; ++i) argv.push_back(fs::path(wide_argv[i]).u8string());
        if (argc == 3 && std::string(argv[1]) == "--grammar-header") {
            auto grammar = json_schema_to_grammar(common_json::parse(kSchema));
            std::ofstream out(fs::u8path(argv[2]), std::ios::binary);
            out << "#pragma once\n// Generated offline by vlm-reference using b12032 json_schema_to_grammar.\n"
                << "inline constexpr char kGrammar[] = R\"GBNF(" << grammar << ")GBNF\";\n";
            return out ? 0 : 1;
        }
        if (argc != 5) throw std::runtime_error("Use MODEL MMPROJ IMAGES OUT, or --grammar-header PATH");
        auto mp = llama_model_default_params();
        mp.vocab_only = true;
        mp.n_gpu_layers = 0;
        auto model = llama_model_load_from_file(argv[1].c_str(), mp);
        if (!model) throw std::runtime_error("Reference model load failed");
        auto tmpls = common_chat_templates_init(model, "");
        common_chat_templates_inputs inputs;
        common_chat_msg message;
        message.role = "user";
        message.content_parts = {{"media_marker", mtmd_default_marker(), ""}, {"text", kPrompt, ""}};
        inputs.messages = {message};
        inputs.enable_thinking = false;
        inputs.reasoning_format = COMMON_REASONING_FORMAT_NONE;
        inputs.chat_template_kwargs["enable_thinking"] = "false";
        inputs.json_schema = kSchema;
        auto applied = common_chat_templates_apply(tmpls.get(), inputs);
        std::string rendered = applied.prompt.str();
        std::cout << "fixed_prompt_equal=" << (rendered == fixed_prompt(mtmd_default_marker()))
            << " grammar_lazy=" << applied.grammar_lazy << '\n';
        if (rendered != fixed_prompt(mtmd_default_marker())) {
            // Contains only the fixed public prompt and synthetic media marker.
            std::cout << "reference_prompt=" << json(rendered).dump() << '\n';
            throw std::runtime_error("Fixed prompt differs");
        }
        // Use the grammar actually returned by the server's template path.
        std::ofstream grammar(fs::u8path(argv[4]).parent_path() / "server-path.gbnf", std::ios::binary);
        grammar << applied.grammar;
        auto vp = mtmd_context_params_default();
        vp.use_gpu = false;
        vp.warmup = false;
        auto vision = mtmd_init_from_file(argv[2].c_str(), model, vp);
        if (!vision) throw std::runtime_error("Reference mmproj load failed");
        std::vector<fs::path> images;
        for (auto & e : fs::directory_iterator(fs::u8path(argv[3]))) if (e.path().extension() == ".png") images.push_back(e.path());
        std::sort(images.begin(), images.end());
        std::ofstream out(fs::u8path(argv[4]), std::ios::binary);
        for (auto & image : images) {
            mtmd_helper_init_opt opt{};
            opt.video_params = mtmd_helper_video_init_params_default();
            auto bmp = mtmd_helper_bitmap_init_from_file(vision, image.u8string().c_str(), false, opt);
            if (!bmp.bitmap) throw std::runtime_error("Reference image load failed");
            auto chunks = mtmd_input_chunks_init();
            const mtmd_bitmap * bitmaps[] = {bmp.bitmap};
            mtmd_input_text text{rendered.data(), rendered.size(), true, true};
            if (mtmd_tokenize(vision, chunks, &text, bitmaps, 1)) throw std::runtime_error("Reference tokenize failed");
            out << json({{"image", image.filename().u8string()}, {"prompt_chunks", describe_chunks(chunks)}}).dump() << '\n';
            mtmd_input_chunks_free(chunks);
            mtmd_bitmap_free(bmp.bitmap);
        }
        mtmd_free(vision);
        llama_model_free(model);
        return out ? 0 : 1;
    } catch (const std::exception & e) { std::cerr << e.what() << '\n'; return 1; }
}
