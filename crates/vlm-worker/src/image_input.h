#pragma once
#include "image_attempt.h"
#include <algorithm>
#include <array>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <limits>
#include <vector>

inline constexpr size_t kMaxPngBytes = 16 * 1024 * 1024;
static_assert(kMaxPngBytes <= static_cast<size_t>((std::numeric_limits<int>::max)()), "STB length must fit int");

inline void check_image_dimensions(uint32_t width, uint32_t height) {
    // Selftest accepts the same pre-resized input as the planned RGB worker.
    if (!width || !height || width > 1024 || height > 1024)
        throw ImageFailure("PNG dimensions must be within 1..1024 on each axis");
}

inline std::vector<unsigned char> read_selftest_png(std::istream & input) {
    std::array<unsigned char, 24> header{};
    if (!input.read(reinterpret_cast<char *>(header.data()), header.size())) throw ImageFailure("Cannot read PNG header");
    const unsigned char prefix[] = {137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 'I', 'H', 'D', 'R'};
    if (!std::equal(std::begin(prefix), std::end(prefix), header.begin())) throw ImageFailure("Invalid PNG header");
    auto u32 = [&](size_t i) {
        return (uint32_t(header[i]) << 24) | (uint32_t(header[i + 1]) << 16) | (uint32_t(header[i + 2]) << 8) | header[i + 3];
    };
    check_image_dimensions(u32(16), u32(20));
    std::vector<unsigned char> bytes(header.begin(), header.end());
    bytes.reserve(kMaxPngBytes);
    std::array<char, 64 * 1024> block{};
    for (;;) {
        size_t remaining = kMaxPngBytes - bytes.size();
        // At most one extra byte is read to distinguish exact-limit input from overflow.
        auto request = static_cast<std::streamsize>((std::min)(block.size(), remaining + 1));
        input.read(block.data(), request);
        auto count = input.gcount();
        if (static_cast<size_t>(count) > remaining) throw ImageFailure("PNG input exceeds 16 MiB byte limit");
        if (input.bad()) throw ImageFailure("Cannot read PNG data");
        bytes.insert(bytes.end(), block.data(), block.data() + count);
        if (input.eof()) break;
        if (input.fail()) throw ImageFailure("Cannot read PNG data");
    }
    // Decode these exact bytes, avoiding a second path open after the header check.
    return bytes;
}

inline std::vector<unsigned char> read_selftest_png(const std::filesystem::path & path) {
    std::ifstream input(path, std::ios::binary);
    return read_selftest_png(input);
}
