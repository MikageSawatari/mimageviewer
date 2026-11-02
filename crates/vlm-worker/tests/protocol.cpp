#include "protocol.h"
#include "prompt.h"
#include <iostream>
#include <limits>
#include <sstream>

int main() {
    using namespace protocol;
    int failures = 0;
    auto check = [&](bool ok) { if (!ok) ++failures; };
    auto invalid = [&](auto && work) {
        try { work(); ++failures; } catch (const Invalid &) {}
    };
    auto encode = [](const std::string & text, const std::string & bytes = "") {
        std::ostringstream out(std::ios::binary);
        write_u32(out, static_cast<uint32_t>(4 + text.size() + bytes.size()));
        write_u32(out, static_cast<uint32_t>(text.size()));
        out << text << bytes;
        return out.str();
    };
    auto decode = [](const std::string & bytes) {
        std::istringstream input(bytes, std::ios::binary);
        Frame frame;
        if (!read(input, frame)) throw Invalid("Unexpected EOF");
        return frame;
    };
    const std::string hello = R"({"type":"hello"})";
    check(validate(decode(encode(hello))) == Kind::Hello);
    const std::string caption = R"({"type":"caption","request_id":18446744073709551615,"width":1,"height":2})";
    const std::string rgb("\0\r\n\x1a\xff\x80", 6);
    auto frame = decode(encode(caption, rgb));
    check(validate(frame) == Kind::Caption);
    check(uint_field(frame.header, "request_id") == (std::numeric_limits<uint64_t>::max)());
    check(std::string(frame.bytes.begin(), frame.bytes.end()) == rgb);
    invalid([&] { validate(decode(encode(caption, rgb.substr(1)))); });
    invalid([&] { validate(decode(encode(hello, rgb))); });
    for (const auto & bad : {std::string("{"), std::string("[]"), hello + "x", hello + std::string(1, '\0'),
        hello + std::string("\xc0\xaf", 2)}) invalid([&] { decode(encode(bad)); });
    for (const char * bad : {R"({"type":"unknown"})", R"({"type":null})", R"({"type":"caption","request_id":-1,"width":1,"height":1})",
        R"({"type":"caption","request_id":18446744073709551616,"width":1,"height":1})",
        R"({"type":"caption","request_id":1.0,"width":1,"height":1})",
        R"({"type":"caption","request_id":1,"width":0,"height":1})",
        R"({"type":"caption","request_id":1,"width":1025,"height":1})"})
        invalid([&] { validate(decode(encode(bad, std::string(3, 'x')))); });
    json load{{"type", "load"}, {"model_path", "日本語.gguf"}, {"mmproj_path", "mmproj.gguf"},
        {"device_luid", "ABCDEF01:01234567"}, {"cpu_threads", 24u}};
    check(validate(decode(encode(load.dump()))) == Kind::Load);
    load["device_luid"] = "";
    check(validate(decode(encode(load.dump()))) == Kind::Load);
    load["cpu_threads"] = 0;
    invalid([&] { validate(decode(encode(load.dump()))); });
    load["cpu_threads"] = 1025;
    invalid([&] { validate(decode(encode(load.dump()))); });
    load["cpu_threads"] = 1;
    load["model_path"] = std::string("x\0y", 3);
    invalid([&] { validate(decode(encode(load.dump()))); });
    load["model_path"] = "model";
    load["device_luid"] = "ZBCDEF01:01234567";
    invalid([&] { validate(decode(encode(load.dump()))); });
    std::istringstream empty;
    Frame eof;
    check(!read(empty, eof));
    auto good = encode(hello);
    for (size_t length = 1; length < good.size(); ++length) invalid([&] { decode(good.substr(0, length)); });
    for (uint32_t length : {0u, 3u, kMaxFrame + 1, UINT32_MAX}) {
        std::ostringstream out; write_u32(out, length);
        invalid([&] { decode(out.str()); });
    }
    for (uint32_t length : {0u, 2u, UINT32_MAX}) {
        std::ostringstream out; write_u32(out, 5); write_u32(out, length); out << 'x';
        invalid([&] { decode(out.str()); });
    }
    // The cap includes the inner prefix. Whitespace padding is valid JSON.
    auto at_limit = hello + std::string(kMaxFrame - 4 - hello.size(), ' ');
    check(validate(decode(encode(at_limit))) == Kind::Hello);
    std::ostringstream response;
    write(response, {{"type", "hello"}, {"protocol_version", kVersion}, {"source_identity", "identity"},
        {"prompt_version", kPromptVersion}});
    auto hello_response = decode(response.str()).header;
    check(hello_response["protocol_version"] == 2);
    check(hello_response["prompt_version"].is_number_integer() && hello_response["prompt_version"] == 1);
    invalid([&] { std::ostringstream out; write(out, {{"large", std::string(kMaxFrame, 'x')}}); });
    Session session;
    invalid([&] { session.accept(Kind::Caption); });
    invalid([&] { session.accept(Kind::Load); });
    session.accept(Kind::Hello);
    invalid([&] { session.accept(Kind::Hello); });
    session.accept(Kind::Load);
    session.accept(Kind::Caption);
    session.accept(Kind::Caption);
    invalid([&] { session.accept(Kind::Load); });
    session.accept(Kind::Shutdown);
    std::cout << "protocol failures=" << failures << '\n';
    return failures ? 1 : 0;
}
