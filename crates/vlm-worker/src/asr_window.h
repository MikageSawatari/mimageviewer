#pragma once
#include <algorithm>
#include <cstdint>
#include <functional>
#include <limits>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

namespace asr {
inline constexpr size_t kRate = 16000;
inline constexpr size_t kWindow = 1800 * kRate;
inline constexpr size_t kSearch = 10 * kRate;
inline constexpr size_t kFollow = 30 * kRate;
inline constexpr size_t kHop = kRate / 100;
inline constexpr size_t kQuietFrames = 50;
inline constexpr size_t kSearchReady = kWindow + kSearch + kQuietFrames / 2 * kHop;
inline constexpr size_t kMaxSamples = kWindow + kSearch + kFollow;
struct Segment {
    uint64_t start_ms, end_ms;
    std::string text; // 採用する区間が決まるまではトークンの生byte列。
};
struct Window {
    uint64_t start_ms, read_end_ms, join_ms;
    bool final;
    std::vector<Segment> segments;
    uint64_t utf8_discarded_bytes = 0, utf8_discarded_fragments = 0;
};

// 途中のbyte列はここが持ち、JSONの置換モードには渡さない。
class Utf8Carry {
    std::string pending_;
    uint64_t discarded_bytes_ = 0, discarded_fragments_ = 0;
    void discard(size_t bytes) { discarded_bytes_ += bytes; ++discarded_fragments_; }
public:
    std::string append(const std::string & bytes) {
        auto text = pending_ + bytes;
        pending_.clear();
        std::string output;
        output.reserve(text.size());
        size_t i = 0;
        while (i < text.size()) {
            auto lead = static_cast<unsigned char>(text[i]);
            size_t n = lead < 0x80 ? 1 : lead >= 0xc2 && lead <= 0xdf ? 2 :
                lead >= 0xe0 && lead <= 0xef ? 3 : lead >= 0xf0 && lead <= 0xf4 ? 4 : 0;
            if (!n) { discard(1); ++i; continue; }
            size_t j = 1;
            for (; j < n && i + j < text.size(); ++j) {
                auto b = static_cast<unsigned char>(text[i + j]);
                if ((b & 0xc0) != 0x80 || (j == 1 && ((lead == 0xe0 && b < 0xa0) ||
                    (lead == 0xed && b >= 0xa0) || (lead == 0xf0 && b < 0x90) || (lead == 0xf4 && b >= 0x90))))
                    break;
            }
            if (j < n) {
                if (i + j == text.size()) { pending_ = text.substr(i); break; }
                // 不一致byteは捨てず、既に一致したprefixだけを1断片と数える。
                // 次の周回でそのbyteを先頭として再評価し、ASCIIや次の正しい文字を保つ。
                discard(j); i += j; continue;
            }
            output.append(text, i, n);
            i += n;
        }
        return output;
    }
    void finish() {
        if (!pending_.empty()) discard(pending_.size());
        pending_.clear();
    }
    uint64_t discarded_bytes() const { return discarded_bytes_; }
    uint64_t discarded_fragments() const { return discarded_fragments_; }
};
// Python str.isspace()の29文字に固定する。U+200B/U+FEFFなどは含めない。
inline bool python_space(uint32_t cp) {
    return (cp >= 9 && cp <= 13) || (cp >= 0x1c && cp <= 0x20) || cp == 0x85 || cp == 0xa0 ||
        cp == 0x1680 || (cp >= 0x2000 && cp <= 0x200a) || cp == 0x2028 || cp == 0x2029 ||
        cp == 0x202f || cp == 0x205f || cp == 0x3000;
}
// 入力はUtf8Carryを通った正当なUTF-8だけ。内部空白を変えず、端の文字だけを除く。
inline std::string trim(std::string text) {
    size_t first = text.size(), last = 0, position = 0;
    while (position < text.size()) {
        const size_t begin = position;
        auto lead = static_cast<unsigned char>(text[position++]);
        const size_t count = lead < 0x80 ? 1 : lead < 0xe0 ? 2 : lead < 0xf0 ? 3 : 4;
        uint32_t cp = count == 1 ? lead : lead & (0x7f >> count);
        for (size_t i = 1; i < count; ++i) cp = (cp << 6) | (static_cast<unsigned char>(text[position++]) & 0x3f);
        if (!python_space(cp)) { if (first == text.size()) first = begin; last = position; }
    }
    return first == text.size() ? "" : text.substr(first, last - first);
}
inline uint64_t sample_ms(uint64_t sample) { return sample / (kRate / 1000); }

// t0_window.py quiet_cutと同じ。10msごとのfloat64平均二乗、50枠の累積和で
// 中央が境界の前後10秒に入る最小値を選ぶ。同点は最初。
inline size_t quiet_cut(const std::vector<float> & pcm) {
    const size_t boundary = kWindow / kHop;
    const size_t lo = boundary - kSearch / kHop - kQuietFrames / 2;
    const size_t hi = std::min(pcm.size() / kHop - kQuietFrames,
        boundary + kSearch / kHop - kQuietFrames / 2);
    std::vector<double> sum(hi - lo + kQuietFrames + 1, 0.0);
    for (size_t frame = lo; frame < hi + kQuietFrames; ++frame) {
        double energy = 0;
        for (size_t i = frame * kHop; i < (frame + 1) * kHop; ++i) {
            const double s = double(pcm[i]) * 32768.0;
            energy += s * s;
        }
        sum[frame - lo + 1] = sum[frame - lo] + energy / kHop;
    }
    double best = std::numeric_limits<double>::infinity();
    size_t selected = lo;
    for (size_t frame = lo; frame <= hi; ++frame) {
        const double energy = sum[frame - lo + kQuietFrames] - sum[frame - lo];
        if (energy < best) { best = energy; selected = frame; }
    }
    return (selected + kQuietFrames / 2) * kHop;
}
inline uint64_t choose_seam(const std::vector<Segment> & segments, uint64_t cut_ms) {
    uint64_t after = std::numeric_limits<uint64_t>::max(), last = cut_ms;
    for (const auto & segment : segments) {
        if (segment.start_ms >= cut_ms) after = std::min(after, segment.start_ms);
        last = std::max(last, segment.end_ms);
    }
    return after == std::numeric_limits<uint64_t>::max() ? last : after;
}

class Stream {
public:
    using Recognize = std::function<std::vector<Segment>(const float *, size_t, uint64_t)>;
    using Publish = std::function<void(Window)>;
    // T0の発話途中の切れ目を偽認識器で検査するための純粋な注入口。
    // 製品の呼出しは省略し、固定のquiet_cutだけを使う。CLI設定には出さない。
    using Cut = std::function<size_t(const std::vector<float> &, uint64_t)>;
private:
    Recognize recognize_;
    Publish publish_;
    Cut select_cut_;
    std::vector<float> pcm_;
    uint64_t start_ = 0, received_ = 0;
    std::optional<size_t> cut_;
    bool finished_ = false;
    size_t next_cut() const { return select_cut_ ? select_cut_(pcm_, start_) : quiet_cut(pcm_); }
    void run(size_t read, bool final) {
        auto segments = recognize_(pcm_.data(), read, sample_ms(start_));
        for (auto & s : segments) {
            if (s.end_ms < s.start_ms) throw std::runtime_error("ASR returned inverted segment timestamps");
            s.start_ms += sample_ms(start_); s.end_ms += sample_ms(start_);
        }
        const uint64_t read_end = sample_ms(start_ + read);
        const uint64_t seam = final ? sample_ms(received_) : choose_seam(segments, sample_ms(start_ + *cut_));
        if (!final && (seam <= sample_ms(start_) || seam > read_end))
            throw std::runtime_error("ASR seam is outside its window");
        segments.erase(std::remove_if(segments.begin(), segments.end(), [&](const Segment & s) {
            return !final && s.start_ms >= seam;
        }), segments.end());
        Utf8Carry utf8;
        for (auto & s : segments) s.text = trim(utf8.append(s.text));
        utf8.finish(); // 別に復号する次の窓へ断片を持ち越さない。
        segments.erase(std::remove_if(segments.begin(), segments.end(), [](const Segment & s) {
            return s.text.empty();
        }), segments.end());
        publish_({sample_ms(start_), read_end, seam, final, std::move(segments), utf8.discarded_bytes(), utf8.discarded_fragments()});
        if (final) { pcm_.clear(); finished_ = true; return; }
        const size_t consumed = static_cast<size_t>(seam * (kRate / 1000) - start_);
        pcm_.erase(pcm_.begin(), pcm_.begin() + consumed);
        start_ += consumed;
        cut_.reset();
    }
public:
    Stream(Recognize recognize, Publish publish, Cut select_cut = {}) : recognize_(std::move(recognize)),
        publish_(std::move(publish)), select_cut_(std::move(select_cut)) {
        pcm_.reserve(kMaxSamples);
    }
    void append(const unsigned char * bytes, size_t count) {
        if (finished_ || count % 2) throw std::runtime_error("Invalid ASR PCM stream state");
        size_t position = 0;
        while (position < count) {
            if (!cut_ && pcm_.size() >= kSearchReady) cut_ = next_cut();
            const size_t target = cut_ ? *cut_ + kFollow : kSearchReady;
            // 次のsampleがあればEOFではない。読み足しのちょうど末尾で終わるときは
            // endまで待ち、長さ0の余計な窓を作らない。
            if (pcm_.size() >= target) { run(target, false); continue; }
            const size_t take = std::min((count - position) / 2, target - pcm_.size());
            for (size_t i = 0; i < take; ++i, position += 2) {
                const uint16_t u = uint16_t(bytes[position]) | uint16_t(bytes[position + 1]) << 8;
                const int32_t s = u < 0x8000 ? int32_t(u) : int32_t(u) - 65536;
                pcm_.push_back(float(s) / 32768.0f);
            }
            received_ += take;
        }
    }
    void end() {
        if (finished_) throw std::runtime_error("ASR stream already ended");
        while (true) {
            if (!cut_ && pcm_.size() > kWindow) cut_ = next_cut();
            const size_t read = cut_ ? std::min(pcm_.size(), *cut_ + kFollow) : pcm_.size();
            const bool final = read == pcm_.size();
            run(read, final);
            if (final) return;
        }
    }
    uint64_t duration_ms() const { return sample_ms(received_); }
    size_t buffered_samples() const { return pcm_.size(); }
};
}
