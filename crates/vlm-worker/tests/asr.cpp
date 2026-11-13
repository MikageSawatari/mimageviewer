#include "asr_protocol.h"
#include "asr_window.h"
#include <iostream>
#include <memory>
#include <sstream>

namespace {
void require(bool test, const char * label) { if (!test) throw std::runtime_error(label); }
template<class F> void rejects(F action) {
    try { action(); } catch (const std::exception &) { return; }
    throw std::runtime_error("Expected rejection");
}
void seams() {
    using asr::Segment;
    require(asr::choose_seam({{1790000,1799000,"a"},{1801000,1805000,"b"}},1800000)==1801000,"next segment seam");
    require(asr::choose_seam({{1790000,1806000,"a"},{1806000,1810000,"b"}},1800000)==1806000,"straddling kept whole");
    require(asr::choose_seam({{1790000,1799000,"a"}},1800000)==1800000,"cut after speech");
    require(asr::choose_seam({{1790000,1812000,"a"}},1800000)==1812000,"long last segment");
    require(asr::choose_seam({},1800000)==1800000,"silent seam");
}
void utf8() {
    asr::Utf8Carry carry;
    require(carry.append("a\xe6") == "a", "hold first byte");
    require(carry.append("\x97") == "", "hold second byte");
    require(carry.append("\xa5\xe6\x9c\xac") == "日本", "join across segments");
    require(carry.append("b\xf0\x9f") == "b", "partial final");
    carry.finish();
    require(carry.append("next") == "next", "discard final partial");
    for (const auto & bytes : {std::string("\x80"),std::string("\xc0\x80"),std::string("\xed\xa0\x80"),
        std::string("\xf4\x90\x80\x80"),std::string("\xe6x")})
        rejects([&] { asr::Utf8Carry invalid; invalid.append(bytes); });
}
void protocol() {
    using namespace asr_protocol;
    Session session;
    auto accept = [&](json header, std::vector<unsigned char> bytes = {}) {
        Frame frame{std::move(header),std::move(bytes)};
        const auto kind = validate(frame);
        session.accept(kind,frame.header,frame.bytes.size());
        if (kind == Kind::End) session.complete_end();
    };
    rejects([&] { accept({{"type","begin"},{"request_id",1u}}); });
    accept({{"type","hello"}});
    rejects([&] { accept({{"type","hello"}}); });
    rejects([&] { accept({{"type","load"},{"model_path","m"},{"device_index",-1}}); });
    accept({{"type","load"},{"model_path","m"},{"device_index",0u}});
    for (uint64_t request : {1u,2u}) {
        accept({{"type","begin"},{"request_id",request}});
        rejects([&] { accept({{"type","pcm"},{"request_id",request+1}}, {0,0}); });
        rejects([&] { accept({{"type","pcm"},{"request_id",request}}, {0}); });
        rejects([&] { accept({{"type","pcm"},{"request_id",request}}, std::vector<unsigned char>(kMaxPcmBytes+2)); });
        accept({{"type","pcm"},{"request_id",request}}, {0,0});
        accept({{"type","end"},{"request_id",request}});
    }
    accept({{"type","shutdown"}});
    std::ostringstream out(std::ios::binary);
    framing::write(out, {{"type","hello"}});
    const auto bytes = out.str();
    for (size_t n = 1; n < bytes.size(); ++n) rejects([&] {
        std::istringstream in(bytes.substr(0,n),std::ios::binary); Frame frame; framing::read(in,frame);
    });
    Frame frame;
    std::istringstream in(bytes,std::ios::binary);
    require(framing::read(in,frame) && frame.header["type"]=="hello","framed response");
    require(!framing::read(in,frame),"clean frame EOF");
}
void feed(asr::Stream & stream, size_t samples, bool speech) {
    std::vector<unsigned char> chunk(1024*1024);
    for (size_t offset = 0; offset < samples;) {
        size_t count = std::min(samples-offset,chunk.size()/2);
        for (size_t i = 0; i < count; ++i) {
            const size_t absolute = offset+i;
            // Same 5-second utterances every 7 seconds as T0's fake whisper.
            const bool loud = speech && absolute >= asr::kRate &&
                ((absolute / asr::kRate - 1) % 7 < 5);
            const uint16_t value = loud ? 3000 : 100;
            chunk[i*2]=static_cast<unsigned char>(value); chunk[i*2+1]=static_cast<unsigned char>(value>>8);
        }
        stream.append(chunk.data(),count*2);
        require(stream.buffered_samples()<=asr::kMaxSamples,"bounded PCM buffer");
        offset+=count;
    }
}
void loop(bool forced) {
    std::vector<std::string> actual;
    size_t runs=0;
    uint64_t last_join=0;
    // t0_follow_test.pyと同じ途中切り。2つの結果を比較せず、実Streamの
    // 次窓への繰越しまで通して、全発話がちょうど1回になることを確かめる。
    const std::vector<uint64_t> cuts_ms{302500,900000,1500500,2101000,3000200};
    std::vector<uint64_t> applied_cuts;
    asr::Stream::Cut select_cut;
    if (forced) select_cut = [&](const std::vector<float> &, uint64_t offset) {
        for (const auto cut : cuts_ms) if (cut > asr::sample_ms(offset) + 1000) {
            applied_cuts.push_back(cut);
            return static_cast<size_t>(cut * (asr::kRate / 1000) - offset);
        }
        throw std::runtime_error("No forced cut remains");
    };
    asr::Stream stream([&](const float *,size_t size,uint64_t offset) {
        ++runs;
        std::vector<asr::Segment> result;
        const uint64_t end=offset+asr::sample_ms(size);
        for (uint64_t a=1000; a+5000<=3994000; a+=7000)
            if(a>=offset && a+5000<=end) result.push_back({a-offset,a+5000-offset,std::to_string(a)});
        if(!result.empty() && asr::sample_ms(size)-result.back().end_ms<1000)result.pop_back();
        return result;
    },[&](asr::Window w) {
        require(w.start_ms==last_join,"consecutive seam ownership");
        last_join=w.join_ms;
        for(const auto & segment:w.segments)actual.push_back(segment.text);
    }, std::move(select_cut));
    feed(stream,4000*asr::kRate,true);
    stream.end();
    std::vector<std::string> expected;
    for(uint64_t a=1000;a+5000<=3994000;a+=7000)expected.push_back(std::to_string(a));
    require(actual==expected,"T0 fake recognizer utterances exactly once");
    require(runs==(forced ? 6 : 3) && stream.duration_ms()==4000000,"bounded windows and exact duration");
    if (forced) require(applied_cuts==cuts_ms,"all five T0 forced cuts used");
}
void eof() {
    // T0 review round 6: 1800.2s, last segment ends exactly at EOF.
    size_t runs=0,windows=0;
    asr::Stream stream([&](const float *,size_t size,uint64_t offset) {
        ++runs;require(offset==0,"EOF single offset");
        require(asr::sample_ms(size)==1800200,"full final audio input");
        return std::vector<asr::Segment>{{0,1785000,"a"},{1785000,1800200,"b"}};
    },[&](asr::Window w) {
        ++windows;require(w.final && w.join_ms==1800200 && w.segments.size()==2,"EOF retains every segment");
    });
    feed(stream,1800*asr::kRate+asr::kRate/5,false);
    require(runs==0,"await EOF before first recognition");
    stream.end();require(runs==1 && windows==1,"no empty EOF window");
    // On an exact read-ahead threshold, wait for end rather than splitting.
    size_t exact_runs=0;
    asr::Stream exact([&](const float *,size_t,uint64_t) { ++exact_runs; return std::vector<asr::Segment>{}; },
        [](asr::Window w) { require(w.final,"exact read-ahead is final"); });
    feed(exact,(1790+30)*asr::kRate,false); // Earliest tie cut is 1790s.
    require(exact_runs==0,"exact threshold not prematurely decoded");
    exact.end();require(exact_runs==1,"exact threshold only one window");
    asr::Stream empty([](const float *,size_t count,uint64_t) {
        require(count==0,"empty input");return std::vector<asr::Segment>{};
    },[](asr::Window w) { require(w.final&&w.join_ms==0,"empty final"); });
    empty.end();require(empty.duration_ms()==0,"empty duration");
}
void failed_item(bool during_append) {
    using namespace asr_protocol;
    Session session;
    auto accept = [&](json header, std::vector<unsigned char> bytes = {}) {
        Frame frame{std::move(header),std::move(bytes)};
        session.accept(validate(frame),frame.header,frame.bytes.size());
        return frame;
    };
    accept({{"type","hello"}});
    accept({{"type","load"},{"model_path","m"},{"device_index",0u}});
    accept({{"type","begin"},{"request_id",10u}});
    size_t recognitions = 0, published = 0;
    auto recognize = [&](const float *, size_t, uint64_t) {
        ++recognitions;
        // Gで観測した形。保留中のe3 83へ新しい文字e3 81 86が続くので、
        // 連結後もUTF-8にならない。置換や黙った削除では直さない。
        if (recognitions == 1) return std::vector<asr::Segment>{{0,100,"a\xe3\x83"},{100,200,"\xe3\x81\x86"}};
        return std::vector<asr::Segment>{{0,500,"次の入力"}};
    };
    auto stream = std::make_unique<asr::Stream>(recognize, [&](asr::Window) { ++published; });
    std::vector<unsigned char> chunk(during_append ? kMaxPcmBytes : 32000, 0);
    uint64_t received_bytes = 0;
    do {
        auto frame = accept({{"type","pcm"},{"request_id",10u}},chunk);
        received_bytes += frame.bytes.size();
        session.run_item([&] { stream->append(frame.bytes.data(),frame.bytes.size()); });
    } while (during_append && !session.draining());
    if (during_append) {
        require(session.draining(),"append UTF-8 failure enters Draining");
        // 失敗した枠の残りも長さへ入り、次のPCMは推論へ渡らない。
        require(received_bytes / 32 > 1820000,"whole failing PCM frame counted");
        stream.reset();
        auto drained = accept({{"type","pcm"},{"request_id",10u}},std::vector<unsigned char>(32000,0));
        received_bytes += drained.bytes.size();
        session.run_item([&] { throw std::runtime_error("Draining decoded more PCM"); });
        rejects([&] { accept({{"type","begin"},{"request_id",11u}}); });
        rejects([&] { accept({{"type","pcm"},{"request_id",11u}},{0,0}); });
        rejects([&] { session.accept_eof(); });
    }
    accept({{"type","end"},{"request_id",10u}});
    session.run_item([&] { stream->end(); });
    const auto failed = session.end_response();
    require(failed["status"]=="item_failure" && failed["code"]=="invalid_utf8", "typed per-input UTF-8 failure");
    require(failed["duration_ms"]==received_bytes/32 && failed["request_id"]==10u,"failed input full duration and ID");
    require(recognitions==1 && published==0,"failed input publishes no invalid window");
    stream.reset();
    session.complete_end();
    require(!session.streaming(),"one terminal end completes failed input");
    rejects([&] { accept({{"type","end"},{"request_id",10u}}); });
    // 同じLoaded sessionと認識器で次の入力を処理する。再load・再試行なし。
    accept({{"type","begin"},{"request_id",11u}});
    stream = std::make_unique<asr::Stream>(recognize, [&](asr::Window w) {
        ++published;require(w.segments.size()==1 && w.segments[0].text=="次の入力","next input text");
    });
    auto good = accept({{"type","pcm"},{"request_id",11u}},std::vector<unsigned char>(32000,0));
    session.run_item([&] { stream->append(good.bytes.data(),good.bytes.size()); });
    accept({{"type","end"},{"request_id",11u}});
    session.run_item([&] { stream->end(); });
    require(session.end_response()["status"]=="ok" && session.end_response()["duration_ms"]==1000u,"next input succeeds");
    session.complete_end();
    session.accept_eof();
    require(recognitions==2 && published==1,"model owner reused after failed input");
    accept({{"type","begin"},{"request_id",12u}});
    rejects([&] { session.run_item([] { throw std::runtime_error("GPU failure must remain fatal"); }); });
    require(!session.draining(),"non-UTF8 failures are not converted to item failures");
}
}
int main() {
    try { seams();utf8();protocol();loop(false);loop(true);eof();failed_item(true);failed_item(false);
        std::cout<<"ASR framing, UTF-8, seams, quiet/forced fake recognizer, EOF and per-input failure continuation passed\n"; }
    catch(const std::exception & e) { std::cerr<<e.what()<<'\n';return 1; }
}
