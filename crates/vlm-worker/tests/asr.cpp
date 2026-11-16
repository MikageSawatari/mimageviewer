#include "asr_protocol.h"
#include "asr_window.h"
#include "device_selection.h"
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
    require(carry.discarded_bytes()==2 && carry.discarded_fragments()==1,"final pending counted once");
    carry.finish();
    require(carry.discarded_bytes()==2 && carry.discarded_fragments()==1,"empty finish adds nothing");
    struct Case { std::string input, expected; uint64_t bytes, fragments; };
    for (const auto & item : std::vector<Case>{
        {"\x80","",1,1},{"\xc0\x80","",2,2},{"\xe0\x80\x80","",3,3},
        {"\xed\xa0\x80","",3,3},{"\xf4\x90\x80\x80","",4,4},
        {"\xe6x","x",1,1},{"\xe3\x83" "A","A",2,1},{"\xc2\xc3\xa9","é",1,1},
        {"a\xff\xfe" "b","ab",2,2},{"\xf0\x9f\x98\x80","😀",0,0}}) {
        asr::Utf8Carry repaired;
        require(repaired.append(item.input)==item.expected,"repair preserves valid bytes and ASCII");
        repaired.finish();
        require(repaired.discarded_bytes()==item.bytes && repaired.discarded_fragments()==item.fragments,"repair fragment definition");
    }
    asr::Utf8Carry observed;
    require(observed.append("\xe3\x83")=="","G fragment initially held");
    require(observed.append("\xe3\x81\x86")=="う","G next valid character reprocessed");
    observed.finish();
    require(observed.discarded_bytes()==2 && observed.discarded_fragments()==1,"G two-byte prefix one fragment");
}
std::string encode(uint32_t cp) {
    if(cp<0x80)return std::string(1,static_cast<char>(cp));
    if(cp<0x800)return {char(0xc0|(cp>>6)),char(0x80|(cp&0x3f))};
    if(cp<0x10000)return {char(0xe0|(cp>>12)),char(0x80|((cp>>6)&0x3f)),char(0x80|(cp&0x3f))};
    return {char(0xf0|(cp>>18)),char(0x80|((cp>>12)&0x3f)),char(0x80|((cp>>6)&0x3f)),char(0x80|(cp&0x3f))};
}
void whitespace() {
    const std::vector<uint32_t> spaces{9,10,11,12,13,0x1c,0x1d,0x1e,0x1f,0x20,0x85,0xa0,0x1680,
        0x2000,0x2001,0x2002,0x2003,0x2004,0x2005,0x2006,0x2007,0x2008,0x2009,0x200a,0x2028,0x2029,0x202f,0x205f,0x3000};
    for(const auto cp:spaces) {
        auto s=encode(cp);
        require(asr::trim(s+"😀"+s)=="😀","Python whitespace removed at both ends");
        require(asr::trim("a"+s+"b")=="a"+s+"b","interior whitespace preserved");
        require(asr::trim(s+s).empty(),"all-whitespace text removed");
    }
    for(const auto cp:{0x200b,0xfeff,0x180e,0x200c,0x200d}) {
        const auto s=encode(cp);require(asr::trim(s+"a"+s)==s+"a"+s,"non-space codepoints preserved");
    }
    size_t matched=0;
    for(uint32_t cp=0;cp<=0x10ffff;++cp)if(asr::python_space(cp))++matched;
    require(matched==29,"exact Python whitespace set");
}
void devices() {
    using json=nlohmann::json;
    const std::string a="00000000:0000000A", b="00000000:0000000B";
    auto row=[](const std::string & luid,const std::string & name,uint32_t index,uint64_t memory,bool software=false) {
        return json{{"luid",luid},{"name",name},{"physical_index",index},{"pci_id",name},
            {"dxgi",{{"software",software},{"dedicated_bytes",memory}}}};
    };
    const auto initial=json::array({row(a,"A",0,8u),row(b,"B",1,0u)});
    const auto reordered=json::array({row(b,"B",0,0u),row(a,"A",1,8u)});
    require(worker_devices::select_inventory_device(initial,a)["physical_index"]==0u,"initial LUID lookup");
    require(worker_devices::select_inventory_device(reordered,"00000000:0000000a")["physical_index"]==1u,"stable LUID survives inventory reorder");
    require(worker_devices::select_inventory_device(reordered,b)["name"]=="B","explicit integrated GPU accepted");
    require(worker_devices::select_inventory_device(reordered,"")["name"]=="A","VLM maximum dedicated memory unchanged");
    rejects([&]{worker_devices::select_inventory_device(json::array({row(b,"B",0,0u)}),"");});
    rejects([&]{worker_devices::select_inventory_device(json::array({row(a,"A",0,8u,true)}),a);});
    rejects([&]{worker_devices::select_inventory_device(reordered,"00000000:0000000C");});
    rejects([&]{worker_devices::select_inventory_device(json::array({row(a,"A",0,8u),row(a,"B",1,8u)}),a);});
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
    for(const auto luid:{"", "00000000-0000000A", "00000000:0000000G", "00000000:0000000"})
        rejects([&] { accept({{"type","load"},{"model_path","m"},{"device_luid",luid}}); });
    rejects([&] { accept({{"type","load"},{"model_path","m"}}); });
    rejects([&] { accept({{"type","load"},{"model_path","m"},{"device_luid","00000000:0000000A"},{"device_index",0u}}); });
    accept({{"type","load"},{"model_path","m"},{"device_luid","00000000:0000000A"}});
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
    // token本文の修復をtransportには適用しない。不正なUTF-8 headerは常に拒否する。
    std::string invalid="{\"type\":\"";invalid.push_back(char(0x80));invalid+="\"}";
    std::ostringstream bad(std::ios::binary);
    framing::write_u32(bad,static_cast<uint32_t>(invalid.size()+4));
    framing::write_u32(bad,static_cast<uint32_t>(invalid.size()));bad<<invalid;
    rejects([&]{std::istringstream input(bad.str(),std::ios::binary);Frame f;framing::read(input,f);});
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
void retained_counts(bool lookahead_only) {
    size_t recognitions=0;
    std::vector<asr::Window> windows;
    asr::Stream stream([&](const float *,size_t,uint64_t) {
        ++recognitions;
        if(recognitions>1)return std::vector<asr::Segment>{{0,100,"　次　"}};
        if(lookahead_only)return std::vector<asr::Segment>{{0,100," 有効 "},{1791000,1791100,"\xff\x80"}};
        // retained部分でGの断片2byte、invalid lead1byte、窓末尾pending2byte。
        // 継ぎ目以降のlookaheadの不正2byteは計数しない。
        return std::vector<asr::Segment>{{0,100," \xe3\x83"},{100,200,"\xe3\x81\x86\xc0\xf0\x9f"},
            {1791000,1791100,"\xff\x80"}};
    },[&](asr::Window w){windows.push_back(std::move(w));});
    feed(stream,1820*asr::kRate+16,false);
    stream.end();
    require(windows.size()==2 && !windows[0].final && windows[1].final,"two counting windows");
    require(windows[0].utf8_discarded_bytes==(lookahead_only?0u:5u) &&
        windows[0].utf8_discarded_fragments==(lookahead_only?0u:3u),"only retained fragments counted");
    require(windows[0].segments.size()==1 && windows[0].segments[0].text==(lookahead_only?"有効":"う"),"retained text repaired then Unicode trimmed");
    require(windows[1].utf8_discarded_bytes==0 && windows[1].utf8_discarded_fragments==0 &&
        windows[1].segments[0].text=="次","window counters independent");
    // 最後の区間がpendingだけで消えても、finishの破棄数は窓に残る。
    asr::Stream pending([](const float *,size_t,uint64_t){return std::vector<asr::Segment>{{0,100,"\xe3\x83"}};},
        [](asr::Window w){require(w.final&&w.segments.empty()&&w.utf8_discarded_bytes==2&&w.utf8_discarded_fragments==1,"pruned final segment still counted");});
    feed(pending,asr::kRate,false);pending.end();
}
void request_lifecycle(bool early_end) {
    using namespace asr_protocol;
    Session session;
    auto accept=[&](json header,const std::vector<unsigned char> & bytes=std::vector<unsigned char>{}) {
        Frame frame{std::move(header),bytes};session.accept(validate(frame),frame.header,frame.bytes.size());
    };
    accept({{"type","hello"}});
    accept({{"type","load"},{"model_path","m"},{"device_luid","00000000:0000000A"}});
    accept({{"type","begin"},{"request_id",10u}});
    size_t recognitions=0;
    std::vector<asr::Window> tentative;
    auto recognize=[&](const float *,size_t,uint64_t) {
        ++recognitions;
        if(recognitions==1)return std::vector<asr::Segment>{{0,100,"a\xe3\x83"},{100,200,"\xe3\x81\x86"}};
        return std::vector<asr::Segment>{{0,100,"次の入力"}};
    };
    auto publish=[&](asr::Window w) {
        session.record_window(w.utf8_discarded_bytes,w.utf8_discarded_fragments);
        tentative.push_back(std::move(w));
    };
    auto stream=std::make_unique<asr::Stream>(recognize,publish);
    const std::vector<unsigned char> chunk(early_end?kMaxPcmBytes:32000,0);
    uint64_t received_bytes=0;
    do {
        accept({{"type","pcm"},{"request_id",10u}},chunk);received_bytes+=chunk.size();
        stream->append(chunk.data(),chunk.size());
    }while(early_end&&tentative.empty());
    rejects([&]{accept({{"type","begin"},{"request_id",11u}});});
    rejects([&]{accept({{"type","pcm"},{"request_id",11u}},{0,0});});
    rejects([&]{session.accept_eof();});
    accept({{"type","end"},{"request_id",10u}});stream->end();
    const auto end=session.end_response();
    require(end["status"]=="ok"&&end["duration_ms"]==received_bytes/32&&end["request_id"]==10u,"early or normal end counts all received PCM");
    uint64_t bytes=0,fragments=0;
    for(const auto & w:tentative){bytes+=w.utf8_discarded_bytes;fragments+=w.utf8_discarded_fragments;}
    require(end["utf8_discarded_bytes"]==bytes&&end["utf8_discarded_fragments"]==fragments&&bytes==2&&fragments==1,"end sums published window counts");
    require(tentative.back().final,"end completes current input without draining state");
    // 抽出失敗は本体が途中endを送り、この入力の暫定窓を捨てる。同じモデルを次へ使う。
    tentative.clear();stream.reset();session.complete_end();
    rejects([&]{accept({{"type","end"},{"request_id",10u}});});
    accept({{"type","begin"},{"request_id",11u}});
    require(session.end_response()["utf8_discarded_bytes"]==0u&&session.end_response()["utf8_discarded_fragments"]==0u,"begin resets request counters");
    stream=std::make_unique<asr::Stream>(recognize,publish);
    const std::vector<unsigned char> good(32000,0);
    accept({{"type","pcm"},{"request_id",11u}},good);stream->append(good.data(),good.size());
    accept({{"type","end"},{"request_id",11u}});stream->end();
    require(session.end_response()["status"]=="ok"&&session.end_response()["duration_ms"]==1000u,"next input succeeds after early end");
    require(session.end_response()["utf8_discarded_bytes"]==0u&&session.end_response()["utf8_discarded_fragments"]==0u,"next input does not inherit repair counts");
    require(tentative.size()==1&&tentative[0].segments[0].text=="次の入力","no tentative result leaks into next input");
    session.complete_end();session.accept_eof();
}
}
int main() {
    try { seams();utf8();whitespace();devices();protocol();loop(false);loop(true);eof();retained_counts(false);retained_counts(true);request_lifecycle(true);request_lifecycle(false);
        std::cout<<"ASR strict framing, UTF-8 repair/counts, Unicode trim, stable LUID, windows and early-end lifecycle passed\n"; }
    catch(const std::exception & e) { std::cerr<<e.what()<<'\n';return 1; }
}
