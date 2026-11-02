#include "serve.h"
#include "protocol.h"
#include "inference.h"
#include "build_identity.h"
#include <fcntl.h>
#include <io.h>
#include <iostream>

int serve() {
    if (_setmode(_fileno(stdin), _O_BINARY) == -1 || _setmode(_fileno(stdout), _O_BINARY) == -1)
        throw std::runtime_error("Cannot put protocol pipes in binary mode");
    protocol::Session session;
    std::unique_ptr<InferenceEngine> engine;
    protocol::Frame frame;
    while (protocol::read(std::cin, frame)) {
        auto kind = protocol::validate(frame);
        session.accept(kind);
        if (kind == protocol::Kind::Shutdown) return 0;
        if (kind == protocol::Kind::Hello) {
            protocol::write(std::cout, {{"type", "hello"}, {"protocol_version", protocol::kVersion},
                {"source_identity", MIV_VLM_SOURCE_SHA256}});
            continue;
        }
        if (kind == protocol::Kind::Load) {
            try {
                engine = std::make_unique<InferenceEngine>(protocol::string_field(frame.header, "model_path"),
                    protocol::string_field(frame.header, "mmproj_path"), protocol::string_field(frame.header, "device_luid"),
                    static_cast<int>(protocol::uint_field(frame.header, "cpu_threads")));
                protocol::write(std::cout, {{"type", "load"}, {"status", "ok"}, {"model_load_s", engine->load_seconds()},
                    {"device", engine->device()}});
            } catch (const std::exception & error) {
                protocol::write(std::cout, {{"type", "load"}, {"status", "fatal"}, {"error", error.what()}});
                return 1;
            }
            continue;
        }
        auto start = Clock::now();
        nlohmann::json rec{{"type", "caption"}, {"request_id", protocol::uint_field(frame.header, "request_id")}};
        bool fatal = false;
        try {
            run_image_attempt(rec, [&] {
                engine->caption_rgb(rec, static_cast<uint32_t>(protocol::uint_field(frame.header, "width")),
                    static_cast<uint32_t>(protocol::uint_field(frame.header, "height")), frame.bytes.data());
            });
            if (rec["status"] == "failed") rec["status"] = "image_failure";
        } catch (const std::exception & error) {
            rec["status"] = "fatal";
            rec["error"] = error.what();
            fatal = true;
        }
        rec["wall_s"] = seconds(start);
        protocol::write(std::cout, rec);
        if (fatal) return 1;
    }
    return 0;
}
