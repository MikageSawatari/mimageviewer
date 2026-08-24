//! シークストリップ (docs/video-seek-strip-plan.md) の実装前提を実素材で確かめる probe。
//!
//! 本体をビルドせずに、次の 3 つだけを測る。
//!
//! 1. **キーフレーム列挙** — コンテナ索引 (`avformat_index_get_entry`) からキーフレーム PTS を
//!    復号なしで全取得できるか。案 I (等幅キーフレーム軸) はこの列が無いと組めない (§9 U2)。
//! 2. **窓の充填** — 窓先頭へ 1 回シークし `skip_frame(NonKey)` で前方復号したとき、11 枚が
//!    どれだけで揃うか。判断基準は「窓確定から可視セルが埋まるまで p90 300ms 未満」(§11)。
//! 3. **波形の窓解析** — 窓 ± pre-roll だけ音声を復号して bins を作るのに何 ms かかるか。
//!    ここが速ければ波形に永続キャッシュは要らない (D7)。
//!
//! usage: seek_strip_probe <video> [<video> ...]

use std::path::Path;
use std::time::Instant;

use ffmpeg_the_third as ffmpeg;

/// ストリップの可視セル数の想定値 (§9 U3 の出発点)。
const WINDOW_CELLS: usize = 11;
/// 可視幅 / 3 画面 raster 幅に加え、利用者が要望した 10 分・30 分の密度も同じ素材で測る。
const WAVE_WINDOW_SPANS_SECS: [f64; 4] = [60.0, 180.0, 600.0, 1800.0];
/// 波形窓の前置き。1 次フィルタの状態を整えるために復号して捨てる (§5.2)。
const WAVE_PREROLL_SECS: f64 = 1.0;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: seek_strip_probe <video> [<video> ...]");
        std::process::exit(2);
    }
    if let Err(e) = ffmpeg::init() {
        eprintln!("ffmpeg init failed: {e}");
        std::process::exit(1);
    }
    // 素材ごとの demuxer 警告で結果が埋もれるので黙らせる。
    ffmpeg::util::log::set_level(ffmpeg::util::log::Level::Fatal);
    for arg in &args {
        let path = Path::new(arg);
        println!("\n================ {} ================", path.display());
        if let Err(e) = probe_one(path) {
            println!("  ERROR: {e}");
        }
    }
}

fn probe_one(path: &Path) -> Result<(), String> {
    let open_t0 = Instant::now();
    let mut ictx = ffmpeg::format::input(path).map_err(|e| format!("open: {e}"))?;
    let open_ms = ms(open_t0);

    let stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| "no video stream".to_string())?;
    let stream_idx = stream.index();
    let tb = stream.time_base();
    let (tb_num, tb_den) = (
        f64::from(tb.numerator()),
        f64::from(tb.denominator()).max(1.0),
    );
    let duration_secs = {
        let d = ictx.duration();
        if d > 0 {
            d as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE)
        } else {
            0.0
        }
    };
    let format_name = ictx.format().name().to_string();
    let codec_id = format!("{:?}", stream.parameters().id());
    println!(
        "  format={format_name} codec={codec_id} duration={duration_secs:.1}s open={open_ms:.1}ms"
    );

    // ── 1. キーフレーム列挙 ────────────────────────────────────────────────
    let enum_t0 = Instant::now();
    let keyframes = enumerate_index_keyframes(&mut ictx, stream_idx, tb_num, tb_den);
    let enum_ms = ms(enum_t0);

    // Matroska は Cues を遅延パースするので、開いた直後の索引は 1 件しか無い。
    // 1 回シークすると demuxer が Cues を読み込むはずなので、そこで数え直す。
    let sparse = keyframes.as_ref().is_none_or(|k| k.len() < 2);
    let keyframes = if sparse {
        let before = keyframes.as_ref().map(|k| k.len()).unwrap_or(0);
        let warm_t0 = Instant::now();
        // SAFETY: ictx は有効。stream_index=-1 は AV_TIME_BASE 単位の指定。
        let seek_ok = unsafe {
            ffmpeg::ffi::av_seek_frame(
                ictx.as_mut_ptr(),
                -1,
                ((duration_secs * 0.5).max(0.0) * 1_000_000.0) as i64,
                ffmpeg::ffi::AVSEEK_FLAG_BACKWARD as i32,
            ) >= 0
        };
        let after = enumerate_index_keyframes(&mut ictx, stream_idx, tb_num, tb_den);
        let warm_ms = ms(warm_t0);
        println!(
            "  [1] index (cold): {before} entries -> seek({}) -> {} entries in {:.1}ms",
            if seek_ok { "ok" } else { "FAILED" },
            after.as_ref().map(|k| k.len()).unwrap_or(0),
            warm_ms
        );
        after
    } else {
        keyframes
    };

    let Some(keyframes) = keyframes else {
        println!("  [1] index: EMPTY -> StripAxis::TimeGrid へフォールバックする素材");
        return Ok(());
    };
    if keyframes.len() < 2 {
        println!(
            "  [1] index: {} entries only -> TimeGrid へフォールバック",
            keyframes.len()
        );
        return Ok(());
    }
    let mut gaps: Vec<f64> = keyframes.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let covered = keyframes.last().copied().unwrap_or(0.0);
    println!(
        "  [1] index: {} keyframes in {:.1}ms  last={:.1}s ({:.0}% of duration)",
        keyframes.len(),
        enum_ms,
        covered,
        if duration_secs > 0.0 {
            covered / duration_secs * 100.0
        } else {
            0.0
        }
    );
    println!(
        "      GOP gap: p50={:.2}s p90={:.2}s max={:.2}s",
        pct(&gaps, 0.5),
        pct(&gaps, 0.9),
        gaps.last().copied().unwrap_or(0.0)
    );

    // ── 2. 窓の充填 ────────────────────────────────────────────────────────
    // 先頭 / 中央 / 後方の 3 か所で測る。先頭だけだと索引や I/O が暖まっていて楽をする。
    // 実装では補助デコーダを動画 1 本につき 1 回だけ開いて使い回すので、probe も同じにする。
    // ファイルオープンを毎窓に含めると、素材によって数百 ms の下駄を履いて判断を誤る。
    match open_window_filler(path, stream_idx) {
        Ok(mut filler) => {
            for (label, frac) in [("head", 0.05_f64), ("mid", 0.5), ("tail", 0.85)] {
                let start_idx = ((keyframes.len() as f64 * frac) as usize)
                    .min(keyframes.len().saturating_sub(WINDOW_CELLS));
                match filler.fill_window(tb_num, tb_den, &keyframes, start_idx) {
                    Ok(r) => println!(
                        "  [2] window {label:>4} @{:>7.1}s: {:2} frames in {:6.1}ms  (seek {:.1}ms, first {:.1}ms, scale {:.1}ms)",
                        keyframes[start_idx],
                        r.frames,
                        r.total_ms,
                        r.seek_ms,
                        r.first_frame_ms,
                        r.scale_ms
                    ),
                    Err(e) => println!("  [2] window {label:>4}: ERROR {e}"),
                }
            }
        }
        Err(e) => println!("  [2] window: ERROR {e}"),
    }

    // ── 3. 波形の窓解析 ────────────────────────────────────────────────────
    for span_secs in WAVE_WINDOW_SPANS_SECS {
        match wave_window(path, duration_secs * 0.5, span_secs) {
            Ok(Some(r)) => println!(
                "  [3] wave window {span_secs:.0}s: decode {:.1}ms + analyze {:.1}ms = {:.1}ms  ({} bins, {} frames)",
                r.decode_ms,
                r.analyze_ms,
                r.decode_ms + r.analyze_ms,
                r.bins,
                r.frames
            ),
            Ok(None) => {
                println!("  [3] wave: no audio stream");
                break;
            }
            Err(e) => println!("  [3] wave {span_secs:.0}s: ERROR {e}"),
        }
    }
    Ok(())
}

/// コンテナ索引からキーフレームの PTS (秒) を取り出す。復号もパケット読みもしない。
fn enumerate_index_keyframes(
    ictx: &mut ffmpeg::format::context::Input,
    stream_idx: usize,
    tb_num: f64,
    tb_den: f64,
) -> Option<Vec<f64>> {
    use ffmpeg::ffi::{
        AVSEEK_FLAG_BACKWARD, avformat_index_get_entries_count, avformat_index_get_entry,
    };

    // SAFETY: stream_idx は `ictx.streams()` から得た有効な index。返る AVStream/AVIndexEntry は
    // ictx が生きている間だけ有効で、この関数内で値をコピーして返す。
    unsafe {
        let fctx = ictx.as_mut_ptr();
        if fctx.is_null() {
            return None;
        }
        let streams = (*fctx).streams;
        if streams.is_null() {
            return None;
        }
        let st = *streams.add(stream_idx);
        if st.is_null() {
            return None;
        }
        let count = avformat_index_get_entries_count(st);
        if count <= 0 {
            return None;
        }
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            let entry = avformat_index_get_entry(st, i);
            if entry.is_null() {
                continue;
            }
            // AVINDEX_KEYFRAME = 1。索引には非キーフレームも入り得るので絞る。
            if (*entry).flags() & 1 == 0 {
                continue;
            }
            out.push((*entry).timestamp as f64 * tb_num / tb_den);
        }
        let _ = AVSEEK_FLAG_BACKWARD; // 使わないが定数の存在確認も兼ねる
        out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        (!out.is_empty()).then_some(out)
    }
}

struct WindowResult {
    frames: usize,
    total_ms: f64,
    seek_ms: f64,
    first_frame_ms: f64,
    scale_ms: f64,
}

/// 窓を充填する補助デコーダ。動画 1 本につき 1 回だけ開き、窓ごとに seek + 前方復号する。
/// 実装の `SeekStripThumbWorker` (§4.3) と同じ寿命にして、開く費用を毎窓に載せない。
struct WindowFiller {
    ictx: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Video,
    stream_idx: usize,
    scaler: Option<ffmpeg::software::scaling::Context>,
}

fn open_window_filler(path: &Path, stream_idx: usize) -> Result<WindowFiller, String> {
    let ictx = ffmpeg::format::input(path).map_err(|e| format!("open: {e}"))?;
    let stream = ictx
        .stream(stream_idx)
        .ok_or_else(|| "stream vanished".to_string())?;
    let ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .map_err(|e| format!("codec ctx: {e}"))?;
    let mut decoder = ctx.decoder().video().map_err(|e| format!("decoder: {e}"))?;
    // ここが要点: 非キーフレームを復号せずに捨てる。
    decoder.skip_frame(ffmpeg::Discard::NonKey);
    Ok(WindowFiller {
        ictx,
        decoder,
        stream_idx,
        scaler: None,
    })
}

impl WindowFiller {
    fn fill_window(
        &mut self,
        tb_num: f64,
        tb_den: f64,
        keyframes: &[f64],
        start_idx: usize,
    ) -> Result<WindowResult, String> {
        use ffmpeg::ffi::{AVSEEK_FLAG_BACKWARD, av_seek_frame};
        use ffmpeg::format::Pixel;
        use ffmpeg::software::scaling::{Context as ScaleContext, Flags as ScaleFlags};
        use ffmpeg::util::frame::video::Video;

        let total_t0 = Instant::now();
        let start_secs = keyframes[start_idx];
        let end_secs = keyframes
            .get(start_idx + WINDOW_CELLS)
            .copied()
            .unwrap_or(f64::INFINITY);

        let seek_t0 = Instant::now();
        // SAFETY: ictx は有効。stream_index=-1 は AV_TIME_BASE 単位の指定を意味する。
        let seek_ok = unsafe {
            av_seek_frame(
                self.ictx.as_mut_ptr(),
                -1,
                (start_secs * 1_000_000.0) as i64,
                AVSEEK_FLAG_BACKWARD as i32,
            ) >= 0
        };
        let seek_ms = ms(seek_t0);
        if !seek_ok {
            return Err("av_seek_frame failed".to_string());
        }
        self.decoder.flush();

        let mut frames = 0usize;
        let mut scale_ms = 0.0_f64;
        let mut first_frame_ms = 0.0_f64;
        let mut frame = Video::empty();

        'demux: for res in self.ictx.packets() {
            let (stream, packet) = res.map_err(|e| format!("demux: {e}"))?;
            if stream.index() != self.stream_idx {
                continue;
            }
            if self.decoder.send_packet(&packet).is_err() {
                continue;
            }
            while self.decoder.receive_frame(&mut frame).is_ok() {
                let pts = frame
                    .timestamp()
                    .unwrap_or_else(|| frame.pts().unwrap_or(0)) as f64
                    * tb_num
                    / tb_den;
                if pts + 1e-3 < start_secs {
                    continue;
                }
                if pts >= end_secs {
                    break 'demux;
                }
                if frames == 0 {
                    first_frame_ms = ms(total_t0);
                }
                // 320px 幅へ縮小するところまで測る (ストリップの抽出幅、§9 U4)。
                let scale_t0 = Instant::now();
                let dst_w = 320u32;
                let dst_h = ((dst_w as f64 * frame.height() as f64 / frame.width().max(1) as f64)
                    .round() as u32)
                    .max(1);
                let sc = match self.scaler.as_mut() {
                    Some(sc) => sc,
                    None => {
                        let made = ScaleContext::get(
                            frame.format(),
                            frame.width(),
                            frame.height(),
                            Pixel::RGBA,
                            dst_w,
                            dst_h,
                            ScaleFlags::BILINEAR,
                        )
                        .map_err(|e| format!("scaler: {e}"))?;
                        self.scaler.insert(made)
                    }
                };
                let mut rgba = Video::empty();
                sc.run(&frame, &mut rgba)
                    .map_err(|e| format!("scale: {e}"))?;
                scale_ms += ms(scale_t0);

                frames += 1;
                if frames >= WINDOW_CELLS {
                    break 'demux;
                }
            }
        }

        Ok(WindowResult {
            frames,
            total_ms: ms(total_t0),
            seek_ms,
            first_frame_ms,
            scale_ms,
        })
    }
}

struct WaveResult {
    decode_ms: f64,
    analyze_ms: f64,
    bins: usize,
    frames: usize,
}

/// 窓 ± pre-roll だけ音声を復号して bins を作る (§5.2 の骨格)。
fn wave_window(
    path: &Path,
    center_secs: f64,
    window_secs: f64,
) -> Result<Option<WaveResult>, String> {
    use ffmpeg::ffi::{AVSEEK_FLAG_BACKWARD, av_seek_frame};
    use ffmpeg::util::frame::audio::Audio;

    const OUT_RATE: u32 = 48_000;

    let decode_t0 = Instant::now();
    let mut ictx = ffmpeg::format::input(path).map_err(|e| format!("open: {e}"))?;
    let Some(stream) = ictx.streams().best(ffmpeg::media::Type::Audio) else {
        return Ok(None);
    };
    let stream_idx = stream.index();
    let tb = stream.time_base();
    let (tb_num, tb_den) = (
        f64::from(tb.numerator()),
        f64::from(tb.denominator()).max(1.0),
    );
    let ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .map_err(|e| format!("codec ctx: {e}"))?;
    let mut decoder = ctx.decoder().audio().map_err(|e| format!("decoder: {e}"))?;

    let window_start = (center_secs - window_secs * 0.5).max(0.0);
    let decode_start = (window_start - WAVE_PREROLL_SECS).max(0.0);
    let window_end = window_start + window_secs;

    // SAFETY: ictx は有効。stream_index=-1 は AV_TIME_BASE 単位。
    let seek_ok = unsafe {
        av_seek_frame(
            ictx.as_mut_ptr(),
            -1,
            (decode_start * 1_000_000.0) as i64,
            AVSEEK_FLAG_BACKWARD as i32,
        ) >= 0
    };
    if !seek_ok {
        return Err("audio av_seek_frame failed".to_string());
    }
    decoder.flush();

    // 本体 `audio_decode::open_audio_decode` と同じ組み方 (48kHz stereo f32 packed)。
    let in_rate = decoder.rate().max(1);
    let in_fmt = decoder.format();
    let in_layout = normalize_layout(decoder.ch_layout());
    let mut resampler = ffmpeg::software::resampling::Context::get2(
        in_fmt,
        in_layout,
        in_rate,
        ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
        ffmpeg::ChannelLayout::STEREO,
        OUT_RATE,
    )
    .map_err(|e| format!("swresample init: {e}"))?;

    let mut stereo: Vec<f32> = Vec::new();
    let mut frame = Audio::empty();
    'demux: for res in ictx.packets() {
        let (stream, packet) = res.map_err(|e| format!("demux: {e}"))?;
        if stream.index() != stream_idx {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            continue;
        }
        while decoder.receive_frame(&mut frame).is_ok() {
            let pts = frame.timestamp().unwrap_or(0) as f64 * tb_num / tb_den;
            if pts > window_end {
                break 'demux;
            }
            append_resampled(&mut resampler, &mut frame, &mut stereo, in_rate, OUT_RATE)?;
        }
    }
    let decode_ms = ms(decode_t0);

    // pre-roll ぶんを捨てる (フィルタ状態を整えるためだけに復号した区間)。
    let drop_frames = ((window_start - decode_start) * OUT_RATE as f64) as usize * 2;
    let analysed = &stereo[drop_frames.min(stereo.len())..];

    let analyze_t0 = Instant::now();
    let config = music_core::AnalysisConfig {
        bin_secs: 0.05,
        ..music_core::AnalysisConfig::default()
    };
    let ta = music_core::analyze_stereo_timeline(analysed, OUT_RATE, config);
    let analyze_ms = ms(analyze_t0);

    Ok(Some(WaveResult {
        decode_ms,
        analyze_ms,
        bins: ta.bins.len(),
        frames: analysed.len() / 2,
    }))
}

/// レイアウト未指定 (古い WMA 等) を既定レイアウトへ差し替える。
/// 本体 `audio_decode::normalize_layout` と同じ。
fn normalize_layout(layout: ffmpeg::ChannelLayout<'_>) -> ffmpeg::ChannelLayout<'static> {
    if layout.mask().is_some() {
        return ffmpeg::ChannelLayout::from(layout.into_owned());
    }
    let channels = layout.channels();
    let substitute = ffmpeg::ChannelLayout::default_for_channels(channels);
    if substitute.mask().is_some() {
        return substitute;
    }
    if channels >= 2 {
        ffmpeg::ChannelLayout::STEREO
    } else {
        ffmpeg::ChannelLayout::MONO
    }
}

/// 本体 `audio_decode::append_resampled` の probe 版 (エラー時の握り潰しだけ簡略化)。
fn append_resampled(
    resampler: &mut ffmpeg::software::resampling::Context,
    frame: &mut ffmpeg::util::frame::audio::Audio,
    out: &mut Vec<f32>,
    in_rate: u32,
    out_rate: u32,
) -> Result<(), String> {
    use ffmpeg::format::{Sample, sample::Type as SampleType};
    use ffmpeg::util::frame::audio::Audio;

    let in_samples = frame.samples();
    if in_samples == 0 {
        return Ok(());
    }
    if frame.ch_layout().mask().is_none() {
        let normalized = normalize_layout(frame.ch_layout());
        frame.set_ch_layout(normalized);
    }
    let delay_out = resampler
        .delay()
        .map(|d| d.output.max(0) as u64)
        .unwrap_or(0);
    let rate_converted = (in_samples as u64 * out_rate as u64).div_ceil(in_rate as u64);
    let out_cap = (rate_converted + delay_out + 32) as usize;
    let mut resampled = Audio::empty();
    // SAFETY: 直後に set_rate して run に渡すだけ。本体と同じ手順。
    unsafe {
        resampled.alloc(
            Sample::F32(SampleType::Packed),
            out_cap,
            ffmpeg::ChannelLayoutMask::STEREO,
        );
        resampled.set_rate(out_rate);
    }
    if resampler.run(frame, &mut resampled).is_err() {
        return Ok(());
    }
    let nb = resampled.samples();
    if nb == 0 {
        return Ok(());
    }
    let count = nb * 2;
    // SAFETY: packed f32 stereo なので data[0] に count 個の f32 が連続する。
    unsafe {
        let ptr = (*resampled.as_ptr()).data[0] as *const f32;
        if ptr.is_null() {
            return Ok(());
        }
        out.extend_from_slice(std::slice::from_raw_parts(ptr, count));
    }
    Ok(())
}

fn ms(t0: Instant) -> f64 {
    t0.elapsed().as_secs_f64() * 1000.0
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let k = (((sorted.len() - 1) as f64) * p).round() as usize;
    sorted[k.min(sorted.len() - 1)]
}
