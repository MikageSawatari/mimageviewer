#include "image_input.h"
#include <iostream>
#include <streambuf>

// A non-seekable logical file, including multi-GiB lengths, without allocating it.
class GeneratedPng : public std::streambuf {
    uint64_t total_;
    uint64_t consumed_ = 0;
    const std::array<unsigned char, 24> header_ = {137, 80, 78, 71, 13, 10, 26, 10,
        0, 0, 0, 13, 'I', 'H', 'D', 'R', 0, 0, 0, 1, 0, 0, 0, 1};
protected:
    std::streamsize xsgetn(char * out, std::streamsize requested) override {
        auto count = static_cast<size_t>((std::min)(uint64_t(requested), total_ - consumed_));
        std::fill(out, out + count, 0);
        if (consumed_ < header_.size()) {
            auto header_count = (std::min)(count, header_.size() - static_cast<size_t>(consumed_));
            std::copy_n(header_.data() + consumed_, header_count, out);
        }
        consumed_ += count;
        return static_cast<std::streamsize>(count);
    }
public:
    explicit GeneratedPng(uint64_t total) : total_(total) {}
    uint64_t consumed() const { return consumed_; }
};

int main() {
    int failures = 0;
    auto check = [&](bool value) { if (!value) ++failures; };
    for (size_t size : {size_t(24), kMaxPngBytes - 1, kMaxPngBytes}) {
        GeneratedPng data(size);
        std::istream input(&data);
        auto bytes = read_selftest_png(input);
        check(bytes.size() == size && bytes.size() <= static_cast<size_t>((std::numeric_limits<int>::max)()));
    }
    int completed = 0;
    for (uint64_t size : {uint64_t(kMaxPngBytes + 1), (uint64_t(1) << 31) + 1, uint64_t(24)}) {
        GeneratedPng data(size);
        std::istream input(&data);
        nlohmann::json record;
        run_image_attempt(record, [&] {
            auto bytes = read_selftest_png(input);
            ++completed;
            record["status"] = "ok";
        });
        if (size > kMaxPngBytes) {
            check(record["status"] == "failed" && record["error"] == "PNG input exceeds 16 MiB byte limit");
            check(data.consumed() == kMaxPngBytes + 1);
        } else check(record["status"] == "ok");
    }
    check(completed == 1); // Oversized inputs do not abort the next image.
    std::cout << "bounded PNG reader failures=" << failures << '\n';
    return failures ? 1 : 0;
}
