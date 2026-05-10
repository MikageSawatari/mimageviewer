//! Phase 0 検証用 CLI — ebur128 audio filter graph で動画の音量を測る。
//!
//! 使い方:
//!   cargo run --release --bin normalize_probe -- path/to/video.mp4
//!
//! 出力:
//!   integrated LUFS と true peak (dBTP) を表示。
//!   frame metadata の全キー / 値も列挙する (= 本実装で読むべきキー名を特定するため)。
//!
//! 比較:
//!   ffmpeg -i path/to/video.mp4 \
//!     -af "aformat=channel_layouts=stereo:sample_fmts=flt:sample_rates=48000,ebur128=peak=true" \
//!     -f null -
//!   (probe と同じ aformat を CLI 側にも噛ませて値を比較する)

use std::path::Path;
use std::time::Instant;

use ffmpeg::format::sample::{Sample, Type as SampleType};
use ffmpeg::media::Type as MediaType;
use ffmpeg::util::frame::audio::Audio;
use ffmpeg_the_third as ffmpeg;

const TARGET_RATE: u32 = 48_000;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: normalize_probe <path/to/video>");
        std::process::exit(2);
    }
    let path = Path::new(&args[1]);
    if !path.exists() {
        eprintln!("File not found: {}", path.display());
        std::process::exit(1);
    }

    if let Err(e) = ffmpeg::init() {
        eprintln!("ffmpeg::init failed: {e}");
        std::process::exit(1);
    }

    match probe(path) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("probe error: {e}");
            std::process::exit(1);
        }
    }
}

fn probe(path: &Path) -> Result<(), String> {
    let t0 = Instant::now();
    let mut input =
        ffmpeg::format::input(&path).map_err(|e| format!("format::input failed: {e}"))?;

    // ── audio stream を選ぶ ──
    let audio_stream = input
        .streams()
        .best(MediaType::Audio)
        .ok_or_else(|| "no audio stream".to_string())?;
    let stream_idx = audio_stream.index();
    let stream_tb = audio_stream.time_base();
    let params = audio_stream.parameters();
    let codec_id = params.id();
    println!(
        "audio stream: idx={stream_idx} codec={} time_base={}/{}",
        codec_id.name(),
        stream_tb.numerator(),
        stream_tb.denominator()
    );

    let ctx = ffmpeg::codec::context::Context::from_parameters(params)
        .map_err(|e| format!("codec context: {e}"))?;
    let mut decoder = ctx
        .decoder()
        .audio()
        .map_err(|e| format!("decoder open: {e}"))?;

    let in_fmt = decoder.format();
    let in_rate = decoder.rate();
    let in_layout = decoder.ch_layout();
    let in_channels = in_layout.channels();
    println!(
        "decoder: in_fmt={:?} in_rate={in_rate} in_channels={in_channels} layout=\"{}\"",
        in_fmt,
        in_layout.description()
    );

    // ── filter graph: abuffer -> aformat=stereo,flt,48000 -> ebur128 -> abuffersink ──
    let mut graph = ffmpeg::filter::Graph::new();

    let buffer = ffmpeg::filter::find("abuffer").ok_or("filter 'abuffer' not found")?;
    let buffersink = ffmpeg::filter::find("abuffersink").ok_or("filter 'abuffersink' not found")?;

    // abuffer args:
    //   time_base=NUM/DEN:sample_rate=N:sample_fmt=NAME:channel_layout=DESC
    let in_fmt_name = sample_fmt_name(in_fmt);
    let layout_desc = in_layout.description();
    let abuffer_args = format!(
        "time_base={}/{}:sample_rate={}:sample_fmt={}:channel_layout={}",
        stream_tb.numerator().max(1),
        stream_tb.denominator().max(1),
        in_rate,
        in_fmt_name,
        layout_desc,
    );
    println!("abuffer args: {abuffer_args}");

    graph
        .add(&buffer, "in", &abuffer_args)
        .map_err(|e| format!("graph add abuffer: {e}"))?;
    graph
        .add(&buffersink, "out", "")
        .map_err(|e| format!("graph add abuffersink: {e}"))?;

    // aformat + ebur128 を parse 経路で追加
    // sample_fmt は "flt" (packed float) を試す。"fltp" にすべきかは Phase 0 検証で確定。
    let chain = format!(
        "aformat=channel_layouts=stereo:sample_fmts=flt:sample_rates={TARGET_RATE},ebur128=metadata=1:peak=true"
    );
    println!("filter chain: {chain}");
    graph
        .output("in", 0)
        .and_then(|p| p.input("out", 0))
        .and_then(|p| p.parse(&chain))
        .map_err(|e| format!("graph parse: {e}"))?;
    graph
        .validate()
        .map_err(|e| format!("graph validate: {e}"))?;

    println!("graph dump:\n{}", graph.dump());

    // ── decode → filter loop ──
    let mut frames_in: u64 = 0;
    let mut frames_out: u64 = 0;
    let mut last_metadata_pairs: Vec<(String, String)> = Vec::new();
    let mut last_seen_keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    let packets: Vec<_> = input
        .packets()
        .filter_map(|r| match r {
            Ok((s, p)) => {
                if s.index() == stream_idx {
                    Some(p)
                } else {
                    None
                }
            }
            Err(e) => {
                eprintln!("packet error (continuing): {e}");
                None
            }
        })
        .collect();
    println!("collected {} audio packets", packets.len());

    for packet in packets {
        if let Err(e) = decoder.send_packet(&packet) {
            eprintln!("send_packet: {e}");
            continue;
        }
        let mut frame = Audio::empty();
        while decoder.receive_frame(&mut frame).is_ok() {
            frames_in += 1;
            push_frame_to_graph(&mut graph, &frame).map_err(|e| format!("push: {e}"))?;
            pull_frames_from_graph(
                &mut graph,
                &mut frames_out,
                &mut last_metadata_pairs,
                &mut last_seen_keys,
            )?;
        }
    }

    // EOF drain: send_packet(NULL) → receive_frame ループ
    use ffmpeg::ffi::avcodec_send_packet;
    unsafe {
        let _ = avcodec_send_packet(decoder.as_mut_ptr(), std::ptr::null());
    }
    let mut frame = Audio::empty();
    while decoder.receive_frame(&mut frame).is_ok() {
        frames_in += 1;
        push_frame_to_graph(&mut graph, &frame).map_err(|e| format!("push (drain): {e}"))?;
        pull_frames_from_graph(
            &mut graph,
            &mut frames_out,
            &mut last_metadata_pairs,
            &mut last_seen_keys,
        )?;
    }

    // filter graph EOF: source に None を流して下流に EOF を伝播させる。
    // av_buffersrc_add_frame(ctx, NULL) で signal、その後 sink から最終 metadata frame を pull。
    unsafe {
        use ffmpeg::ffi::av_buffersrc_add_frame;
        let mut src = graph
            .get("in")
            .ok_or_else(|| "graph 'in' missing".to_string())?;
        let _ = av_buffersrc_add_frame(src.as_mut_ptr(), std::ptr::null_mut());
    }
    pull_frames_from_graph(
        &mut graph,
        &mut frames_out,
        &mut last_metadata_pairs,
        &mut last_seen_keys,
    )?;

    let elapsed = t0.elapsed();
    println!("\n=== summary ===");
    println!(
        "frames_in (decoder out)   = {frames_in}\nframes_out (filter sink) = {frames_out}\nelapsed = {:.2}s",
        elapsed.as_secs_f64()
    );
    println!("\nall metadata keys observed across frames (sorted):");
    for k in &last_seen_keys {
        println!("  {k}");
    }
    println!("\nlast frame metadata key/value:");
    for (k, v) in &last_metadata_pairs {
        println!("  {k} = {v}");
    }

    Ok(())
}

fn push_frame_to_graph(
    graph: &mut ffmpeg::filter::Graph,
    frame: &Audio,
) -> Result<(), ffmpeg::Error> {
    let mut src = graph.get("in").expect("graph 'in' must exist");
    src.source().add(frame)
}

fn pull_frames_from_graph(
    graph: &mut ffmpeg::filter::Graph,
    frames_out: &mut u64,
    last_metadata_pairs: &mut Vec<(String, String)>,
    last_seen_keys: &mut std::collections::BTreeSet<String>,
) -> Result<(), String> {
    const EAGAIN_ERRNO: i32 = 11; // libc::EAGAIN on Windows MSVC
    loop {
        let mut out = Audio::empty();
        let mut sink = graph
            .get("out")
            .ok_or_else(|| "graph 'out' missing".to_string())?;
        match sink.sink().frame(&mut out) {
            Ok(()) => {
                *frames_out += 1;
                let md = out.metadata();
                let mut pairs: Vec<(String, String)> = Vec::new();
                for (k, v) in md.iter() {
                    last_seen_keys.insert(k.to_string());
                    pairs.push((k.to_string(), v.to_string()));
                }
                if !pairs.is_empty() {
                    *last_metadata_pairs = pairs;
                }
            }
            Err(ffmpeg::Error::Other { errno }) if errno == EAGAIN_ERRNO => break,
            Err(ffmpeg::Error::Eof) => break,
            Err(e) => return Err(format!("sink frame: {e}")),
        }
    }
    Ok(())
}

fn sample_fmt_name(fmt: Sample) -> &'static str {
    match fmt {
        Sample::None => "none",
        Sample::U8(SampleType::Packed) => "u8",
        Sample::U8(SampleType::Planar) => "u8p",
        Sample::I16(SampleType::Packed) => "s16",
        Sample::I16(SampleType::Planar) => "s16p",
        Sample::I32(SampleType::Packed) => "s32",
        Sample::I32(SampleType::Planar) => "s32p",
        Sample::I64(SampleType::Packed) => "s64",
        Sample::I64(SampleType::Planar) => "s64p",
        Sample::F32(SampleType::Packed) => "flt",
        Sample::F32(SampleType::Planar) => "fltp",
        Sample::F64(SampleType::Packed) => "dbl",
        Sample::F64(SampleType::Planar) => "dblp",
    }
}
