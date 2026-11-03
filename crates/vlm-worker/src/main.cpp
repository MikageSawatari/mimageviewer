#include "devices.h"
#include "inference.h"
#include "serve.h"
#include "exclusive_output.h"
#include "prompt.h"
#include "build_identity.h"
#include <algorithm>
#include <filesystem>
#include <iostream>
#include <map>
#include <cctype>
#include <stdexcept>
#include <vector>

using json = nlohmann::json;
namespace fs = std::filesystem;
int run(const std::vector<std::string> & args) {
    std::cerr << MIV_VLM_SOURCE_MARKER << '\n';
    route_engine_logs_to_stderr();
    if (args.size() == 1 && args[0] == "--serve") return serve();
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
    if (!selftest) throw std::runtime_error("Use --selftest --model GGUF --mmproj GGUF --images DIR --out JSONL [--device HIGH:LOW] [--order forward|reverse], or --list-devices / --serve");
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
    InferenceEngine engine(options["--model"], options["--mmproj"], options["--device"], 24);
    double load_s = engine.load_seconds();
    size_t failed = 0;
    for (size_t index = 0; index < images.size(); ++index) {
        auto t0 = Clock::now();
        json rec{{"label", "V1-0"}, {"prompt_version", kPromptEvaluationLabel}, {"image", images[index].filename().u8string()},
            {"index", index}, {"order", order}, {"model_load_s", load_s}, {"device", engine.device()}};
        run_image_attempt(rec, [&] {
            engine.caption_png(rec, images[index]);
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
