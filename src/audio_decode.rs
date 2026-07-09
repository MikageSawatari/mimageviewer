//! 音声ファイルを解析用に「全尺デコード → インターリーブ stereo f32 / 48kHz」する
//! オフラインデコーダ。
//!
//! 再生は本体の `VideoPlayer` (音声のみでも可) を再利用するが、タイムライン解析は
//! 再生ペーシングと独立した専用ワーカーで走らせたい (docs/music-integration-plan.md
//! §5.3 / D9)。ここでは FFmpeg (avformat + avcodec + swresample) で 1 ファイルを丸ごと
//! デコードし、`music_core::analyze_stereo_timeline` が要求する
//! **インターリーブ stereo f32 PCM** を作る。
//!
//! swresample から packed f32 を取り出す手順は動画側 `video/decoder.rs` の実績ある
//! 実装 (linesize パディングを raw pointer で回避する) をそのまま踏襲する。
//!
//! ⚠️ 実際のデコード動作は FFmpeg DLL + 実ファイルが要るため実機検証項目。純ロジックの
//! 解析 (`analyze_stereo_timeline`) と DB は機械なしでテスト済み。

use std::path::Path;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ffmpeg::format::sample::{Sample, Type as SampleType};
use ffmpeg::software::resampling::Context as ResampleContext;
use ffmpeg::util::frame::Audio as AudioFrame;
use ffmpeg_the_third as ffmpeg;
use music_core::{AudioStreamInfo, DecodedAudio};

/// 解析用の出力サンプルレート (Hz)。動画音声パイプラインと揃えて 48kHz stereo にする。
/// progressive spectrum PCM の容量先取り (`run_music_analysis`) でも参照するため crate 公開。
pub(crate) const OUT_RATE: u32 = 48_000;
const OUT_CHANNELS: usize = 2;

static FFMPEG_INIT: Once = Once::new();

fn ensure_ffmpeg_init() {
    FFMPEG_INIT.call_once(|| {
        let _ = ffmpeg::init();
    });
}

/// 音声ファイルをデコードして `music_core` のタイムライン解析結果を返す。
///
/// decode (FFmpeg) → `analyze_stereo_timeline` (純ロジック) の合成。DB キャッシュの
/// 参照・保存は呼び出し側 (Inc 3 の解析ワーカー) の責務にして、この関数はステートレスに
/// 保つ。`cancel` は decode 中と解析直前で確認する。
pub fn analyze_audio_file(
    path: &Path,
    cancel: &AtomicBool,
) -> Result<music_core::TimelineAnalysis, String> {
    analyze_audio_file_with_config(path, cancel, music_core::AnalysisConfig::default())
}

/// `analyze_audio_file` の解析 config を明示する版。音楽ビュー (Inc 3) はラボと同じ
/// 細かい bin (`bin_secs = 0.010`) でタイムラインを描くため、default より高解像度の
/// config を渡す。decode → `analyze_stereo_timeline` の合成は共通。
pub fn analyze_audio_file_with_config(
    path: &Path,
    cancel: &AtomicBool,
    config: music_core::AnalysisConfig,
) -> Result<music_core::TimelineAnalysis, String> {
    let decoded = decode_audio_file_to_stereo_f32(path, cancel)?;
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".to_string());
    }
    Ok(music_core::analyze_stereo_timeline(
        &decoded.stereo_samples,
        decoded.info.sample_rate,
        config,
    ))
}

/// progressive partial emit の最小 wall-clock 間隔。デコードが realtime より速いと 2/4/8 秒の
/// マイルストーンが一気に来るので、これで 1 emit/interval に間引く。
const PARTIAL_EMIT_MIN_INTERVAL: Duration = Duration::from_millis(150);

/// 最初の progressive partial を emit する蓄積秒数。これ以降は倍々。
const PARTIAL_INITIAL_SECS: f64 = 2.0;

/// 次に progressive partial を emit すべき蓄積 frame 数（幾何級数 = 倍々）を返す純関数。
/// `last_emit_frames` = 直近 emit 時の frame 数（初回 emit 前は 0）。初回は `initial_frames`、
/// 以降は前回の 2 倍。これで partial の再解析総コストが「全長の約 2 倍」で頭打ちになる
/// （線形に毎 N 秒だと O(n^2)）。
fn next_partial_threshold_frames(last_emit_frames: usize, initial_frames: usize) -> usize {
    if last_emit_frames == 0 {
        initial_frames.max(1)
    } else {
        last_emit_frames.saturating_mul(2)
    }
}

/// progressive partial timeline emit のスケジューラ。幾何級数マイルストーン
/// (最初 `PARTIAL_INITIAL_SECS`、以降 frame 数を倍々) + wall-clock throttle
/// (`PARTIAL_EMIT_MIN_INTERVAL`) + 全長 50% 抑制。デコード関数の内側と、共有バッファへ
/// 追記しながら partial を出す解析ワーカー (`run_music_analysis`) の両方から使えるよう、
/// スケジュール状態を 1 構造体に閉じる (docs/music-integration-plan.md §11)。
pub(crate) struct PartialEmitSchedule {
    initial_frames: usize,
    /// 全長の 50% を超えたら partial を止める閾値 (final がすぐ後を追うため)。全長不明なら MAX。
    half_frames: usize,
    last_emit_frames: usize,
    last_emit_at: Option<Instant>,
}

impl PartialEmitSchedule {
    pub(crate) fn new(total_duration_secs: f64) -> Self {
        let initial_frames = (PARTIAL_INITIAL_SECS * OUT_RATE as f64) as usize;
        let half_frames = if total_duration_secs > 0.0 {
            (total_duration_secs * 0.5 * OUT_RATE as f64) as usize
        } else {
            usize::MAX
        };
        Self {
            initial_frames,
            half_frames,
            last_emit_frames: 0,
            last_emit_at: None,
        }
    }

    /// `frames` 蓄積時点で partial を emit すべきか (純検査、状態は進めない)。true を返したら
    /// 呼び出し側は on_partial (解析 + 送信) を実行し、**その完了直後**に `record_emitted(frames)`
    /// を呼ぶ。throttle 起点を「emit 完了時刻」にすることで、解析が重い場合でも旧挙動 (on_partial の
    /// 後で 150ms 計測) を保つ。
    pub(crate) fn should_emit(&self, frames: usize) -> bool {
        let threshold = next_partial_threshold_frames(self.last_emit_frames, self.initial_frames);
        frames >= threshold
            && frames < self.half_frames
            && self
                .last_emit_at
                .is_none_or(|t| t.elapsed() >= PARTIAL_EMIT_MIN_INTERVAL)
    }

    /// on_partial 実行直後に呼び、次マイルストーン基準と throttle 起点を進める。
    pub(crate) fn record_emitted(&mut self, frames: usize) {
        self.last_emit_frames = frames;
        self.last_emit_at = Some(Instant::now());
    }
}

/// 開いた音声デコーダ一式 (input context + 選択ストリーム + decoder + resampler)。
/// `decode_audio_file_to_stereo_f32_streaming` と `decode_audio_file_progressive` で共有する
/// セットアップ (avformat open → best audio stream → decoder → 48kHz stereo resampler)。
struct AudioDecodeCtx {
    ictx: ffmpeg::format::context::Input,
    stream_index: usize,
    decoder: ffmpeg::decoder::Audio,
    resampler: ResampleContext,
    in_rate: u32,
}

/// 音声ファイルを開き、最良の音声ストリーム向けに decoder と 48kHz stereo f32 packed への
/// resampler を構築する。レイアウト未指定 (古い WMA 等) は `normalize_layout` で差し替える。
fn open_audio_decode(path: &Path) -> Result<AudioDecodeCtx, String> {
    ensure_ffmpeg_init();

    let pb = path.to_path_buf();
    let ictx = ffmpeg::format::input(&pb).map_err(|e| format!("format::input: {e}"))?;

    // 最良の音声ストリームを選ぶ。`stream.parameters()` は stream を借用するので、
    // codec context の構築まで stream スコープ内で済ませてから owned な context を取り出す。
    let (stream_index, codec_ctx) = {
        let stream = ictx
            .streams()
            .best(ffmpeg::media::Type::Audio)
            .ok_or_else(|| "音声ストリームが見つかりません".to_string())?;
        let idx = stream.index();
        let ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .map_err(|e| format!("codec context: {e}"))?;
        (idx, ctx)
    };
    let decoder = codec_ctx
        .decoder()
        .audio()
        .map_err(|e| format!("audio decoder open: {e}"))?;

    let in_fmt = decoder.format();
    let in_rate = decoder.rate();
    // レイアウト未指定 (AV_CHANNEL_ORDER_UNSPEC、古い WMA 等) だと swresample::get2 が
    // 内部の mask().unwrap() で panic するので、チャンネル数から既定レイアウトへ差し替える。
    let in_layout = normalize_layout(decoder.ch_layout());

    let resampler = ResampleContext::get2(
        in_fmt,
        in_layout,
        in_rate,
        Sample::F32(SampleType::Packed),
        ffmpeg::ChannelLayout::STEREO,
        OUT_RATE,
    )
    .map_err(|e| format!("swresample init: {e}"))?;

    Ok(AudioDecodeCtx {
        ictx,
        stream_index,
        decoder,
        resampler,
        in_rate,
    })
}

/// 音声ファイルを全尺デコードして 48kHz interleaved stereo f32 PCM を返す。
///
/// `cancel` が立ったら途中で `Err` を返して打ち切る (呼び出し側ワーカーが結果を破棄する)。
pub fn decode_audio_file_to_stereo_f32(
    path: &Path,
    cancel: &AtomicBool,
) -> Result<DecodedAudio, String> {
    decode_audio_file_to_stereo_f32_streaming(path, cancel, 0.0, |_, _| {})
}

/// `decode_audio_file_to_stereo_f32` の progressive 版。デコードが進むたびに、蓄積した
/// プレフィックス PCM を `on_partial(&prefix, sample_rate)` で呼び出し側へ渡す。音楽ビューの
/// タイムラインを「全尺デコード完了を待たず順次表示」するために使う
/// （docs/music-integration-plan.md Inc 3 ストリーミング）。
///
/// 呼び出し頻度は **幾何級数マイルストーン**（最初 `PARTIAL_INITIAL_SECS` 秒、以降 frame 数を
/// 倍々）+ wall-clock throttle（`PARTIAL_EMIT_MIN_INTERVAL`）で間引く。`total_duration_secs > 0.0`
/// のとき、蓄積が全長の 50% を超えたら on_partial 呼び出しを止める（final フル解析がすぐ後を
/// 追うため）。再解析総コスト（呼び出し側が各 partial を解析する場合）: duration 既知時は
/// partial 合計 `<T` + final `T` = 全長の **約 2x 以内**、duration 未知（`<= 0.0`、抑制なし）時は
/// partial 合計 `<2T` + final `T` = **最悪 約 3x**（Codex P3）。実運用は probe で duration が取れる
/// ため通常は 2x 側。`on_partial` に渡す `&[f32]` は蓄積中の buffer への借用（ゼロコピー）。
/// 呼び出し側はここで解析して即 send し、借用を跨いで保持しない。
pub fn decode_audio_file_to_stereo_f32_streaming(
    path: &Path,
    cancel: &AtomicBool,
    total_duration_secs: f64,
    mut on_partial: impl FnMut(&[f32], u32),
) -> Result<DecodedAudio, String> {
    let AudioDecodeCtx {
        mut ictx,
        stream_index,
        mut decoder,
        mut resampler,
        in_rate,
    } = open_audio_decode(path)?;

    let mut out: Vec<f32> = Vec::new();
    let mut frame = AudioFrame::empty();

    // progressive partial emit のスケジュール状態 (幾何級数 + wall-clock throttle + 50% 抑制)。
    let mut schedule = PartialEmitSchedule::new(total_duration_secs);

    for res in ictx.packets() {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".to_string());
        }
        let (stream, packet) = res.map_err(|e| format!("demux: {e}"))?;
        if stream.index() != stream_index {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            // 1 packet の decode 失敗は致命的でない (壊れフレーム等) ので継続。
            continue;
        }
        drain_decoder(&mut decoder, &mut frame, &mut resampler, &mut out, in_rate)?;

        // progressive partial: 蓄積プレフィックス全体を呼び出し側へ渡す (ゼロコピー借用)。
        let frames = out.len() / OUT_CHANNELS;
        if schedule.should_emit(frames) {
            on_partial(&out, OUT_RATE);
            schedule.record_emitted(frames);
        }
    }

    // EOF: decoder に溜まった残りフレームを drain し、resampler の内部 delay も吐き切る。
    let _ = decoder.send_eof();
    drain_decoder(&mut decoder, &mut frame, &mut resampler, &mut out, in_rate)?;
    flush_resampler(&mut resampler, &mut out)?;

    let frame_count = out.len() / OUT_CHANNELS;
    let duration_secs = frame_count as f64 / OUT_RATE as f64;

    Ok(DecodedAudio {
        info: AudioStreamInfo {
            sample_rate: OUT_RATE,
            channels: OUT_CHANNELS as u16,
            duration_secs,
        },
        stereo_samples: out,
    })
}

/// 音楽ビュー下段スペクトラム (Inc 4) 用の progressive デコード。
///
/// `decode_audio_file_to_stereo_f32_streaming` と違い、最終 PCM をこの関数内に蓄積して返さず、
/// フレーム drain ごとに **新たにデコードした差分**だけを `on_delta(&new_samples, sample_rate)`
/// で呼び出し側へ渡す (ゼロコピー借用、`&[f32]` は次の drain まで有効)。呼び出し側
/// (`run_music_analysis`) はこの差分を共有 `MusicPcm` バッファへ末尾 append するので、全尺
/// デコード完了を待たず、再生位置がデコード済み範囲にあれば下段スペクトラムが即描画できる
/// (docs/music-integration-plan.md §5.6/§11)。この関数は PCM を保持しない (= 呼び出し側の
/// 共有バッファと二重常駐しない)。timeline の progressive 先出し / 最終確定は呼び出し側が共有
/// バッファのプレフィックスから行う (二重デコードなし)。`cancel` は decode 中に確認する。
///
/// `on_delta` が `Err` を返したら (呼び出し側の共有バッファ確保失敗など) その場でデコードを
/// 打ち切り、その `Err` をこの関数の `Err` として返す (長尺 OOM を abort させず上位へ返す)。
/// 返り値 (`Ok`) は `AudioStreamInfo` (実サンプル数から算出した長さ等)。
pub fn decode_audio_file_progressive(
    path: &Path,
    cancel: &AtomicBool,
    mut on_delta: impl FnMut(&[f32], u32) -> Result<(), String>,
) -> Result<AudioStreamInfo, String> {
    let AudioDecodeCtx {
        mut ictx,
        stream_index,
        mut decoder,
        mut resampler,
        in_rate,
    } = open_audio_decode(path)?;

    let mut frame = AudioFrame::empty();
    // drain した差分だけを載せる再利用スクラッチ (差分を on_delta へ渡すたび clear)。
    let mut scratch: Vec<f32> = Vec::new();
    let mut total_frames = 0usize;

    for res in ictx.packets() {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".to_string());
        }
        let (stream, packet) = res.map_err(|e| format!("demux: {e}"))?;
        if stream.index() != stream_index {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            // 1 packet の decode 失敗は致命的でない (壊れフレーム等) ので継続。
            continue;
        }
        scratch.clear();
        drain_decoder(
            &mut decoder,
            &mut frame,
            &mut resampler,
            &mut scratch,
            in_rate,
        )?;
        if !scratch.is_empty() {
            total_frames += scratch.len() / OUT_CHANNELS;
            on_delta(&scratch, OUT_RATE)?;
        }
    }

    // EOF: decoder に溜まった残りフレームを drain し、resampler の内部 delay も吐き切る。
    let _ = decoder.send_eof();
    scratch.clear();
    drain_decoder(
        &mut decoder,
        &mut frame,
        &mut resampler,
        &mut scratch,
        in_rate,
    )?;
    flush_resampler(&mut resampler, &mut scratch)?;
    if !scratch.is_empty() {
        total_frames += scratch.len() / OUT_CHANNELS;
        on_delta(&scratch, OUT_RATE)?;
    }

    Ok(AudioStreamInfo {
        sample_rate: OUT_RATE,
        channels: OUT_CHANNELS as u16,
        duration_secs: total_frames as f64 / OUT_RATE as f64,
    })
}

/// 入力レイアウトを swresample が扱える (mask を持つ native order の) レイアウトへ
/// 正規化する。mask を持たない (UNSPEC) 入力はチャンネル数から既定レイアウトへ差し替える。
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

/// 音声ファイルのソースメタ情報 (avformat probe)。
///
/// 再生用デコード (`decode_audio_file_to_stereo_f32`) は 48kHz stereo に正規化するので、
/// そこから取れる sample_rate / channels は**ソース値ではない**。右パネルの「音楽情報」
/// 表示にはソースの container / codec / 実 sample rate / channels / bitrate / duration /
/// 埋め込みメタタグが要るため、デコードとは別にこの軽量 probe (ヘッダ読みのみ・PCM
/// デコードしない) で取る。埋め込みメタは FFmpeg avformat の標準メタデータ
/// (CLAUDE.md「外部ツール名の非言及」に従い実装詳細のみ)。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioProbe {
    /// コンテナ形式の説明 (取れなければ短縮名)。
    pub format_name: String,
    /// 音声コーデック名。
    pub codec_name: String,
    /// ソースのサンプルレート (Hz)。
    pub sample_rate: u32,
    /// ソースのチャンネル数。
    pub channels: u16,
    /// 平均ビットレート (bps)。コンテナ値優先、無ければストリーム値。
    pub bit_rate_bps: i64,
    /// 長さ (秒)。
    pub duration_secs: f64,
    /// 埋め込みメタタグ (表示ラベル, 値) を表示順で保持。
    pub tags: Vec<(String, String)>,
}

/// 表示する埋め込みメタの (メタキー小文字, 日本語ラベル) を表示順で並べたもの。
const AUDIO_PROBE_TAG_KEYS: &[(&str, &str)] = &[
    ("title", "タイトル"),
    ("artist", "アーティスト"),
    ("album", "アルバム"),
    ("album_artist", "アルバムアーティスト"),
    ("composer", "作曲"),
    ("date", "年"),
    ("genre", "ジャンル"),
    ("track", "トラック"),
    ("disc", "ディスク"),
    ("publisher", "発行"),
    ("comment", "コメント"),
];

/// 音声ファイルのソースメタ情報を probe する (PCM デコードしない軽量読み)。
///
/// UI スレッドから直接呼ばず、解析ワーカー (`run_music_analysis`) の背景スレッドで実行する
/// こと (avformat の open + ヘッダ読みはブロッキング I/O)。
pub fn probe_audio_file(path: &Path) -> Result<AudioProbe, String> {
    ensure_ffmpeg_init();

    let pb = path.to_path_buf();
    let ictx = ffmpeg::format::input(&pb).map_err(|e| format!("format::input: {e}"))?;

    // メタデータ (キー小文字, 値) を format-level + audio stream-level から集める。
    // FLAC / OGG などはタグをストリーム側に持つので両方拾う。format が優先されるよう
    // format-level を先に入れ、重複キーは最初の値を採用する。
    let mut meta: Vec<(String, String)> = Vec::new();
    let mut push_meta = |k: &str, v: &str| {
        let key = k.trim().to_lowercase();
        if key.is_empty() || v.trim().is_empty() {
            return;
        }
        if meta.iter().any(|(existing, _)| existing == &key) {
            return;
        }
        meta.push((key, v.trim().to_string()));
    };
    for (k, v) in ictx.metadata().iter() {
        push_meta(k, v);
    }

    // 音声ストリームから codec / sample rate / channels / stream bitrate を取る。
    let (codec_name, sample_rate, channels, stream_bit_rate) = {
        let stream = ictx
            .streams()
            .best(ffmpeg::media::Type::Audio)
            .ok_or_else(|| "音声ストリームが見つかりません".to_string())?;
        for (k, v) in stream.metadata().iter() {
            push_meta(k, v);
        }
        let params = stream.parameters();
        let stream_bit_rate = params.bit_rate();
        let ctx = ffmpeg::codec::context::Context::from_parameters(params)
            .map_err(|e| format!("codec context: {e}"))?;
        let codec_name = ctx.id().name().to_string();
        let dec = ctx
            .decoder()
            .audio()
            .map_err(|e| format!("audio decoder: {e}"))?;
        (
            codec_name,
            dec.rate(),
            dec.ch_layout().channels() as u16,
            stream_bit_rate,
        )
    };

    let format_name = {
        let f = ictx.format();
        let desc = f.description();
        if desc.is_empty() {
            f.name().to_string()
        } else {
            desc.to_string()
        }
    };
    let duration = ictx.duration();
    let duration_secs = if duration > 0 {
        duration as f64 / 1_000_000.0
    } else {
        0.0
    };
    let container_bit_rate = ictx.bit_rate();
    let bit_rate_bps = if container_bit_rate > 0 {
        container_bit_rate
    } else {
        stream_bit_rate
    };

    let tags = AUDIO_PROBE_TAG_KEYS
        .iter()
        .filter_map(|(key, label)| {
            meta.iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| (label.to_string(), v.clone()))
        })
        .collect();

    Ok(AudioProbe {
        format_name,
        codec_name,
        sample_rate,
        channels,
        bit_rate_bps,
        duration_secs,
        tags,
    })
}

/// decoder に溜まったフレームを `receive_frame` で全て取り出し、resample して `out` へ。
fn drain_decoder(
    decoder: &mut ffmpeg::decoder::Audio,
    frame: &mut AudioFrame,
    resampler: &mut ResampleContext,
    out: &mut Vec<f32>,
    in_rate: u32,
) -> Result<(), String> {
    while decoder.receive_frame(frame).is_ok() {
        append_resampled(resampler, frame, out, in_rate)?;
    }
    Ok(())
}

/// swresample の出力バッファ確保に足す安全マージン (delay 見積りの端数吸収)。
const SWR_OUTPUT_SAFETY_SAMPLES: u64 = 32;

/// EOF 後に swresample の内部 delay に残ったサンプルを吐き切って `out` へ追記する。
///
/// 通常レート変換の delay は数十サンプルで解析への影響は無視できるが、極短ファイル
/// (数千サンプル級) では末尾の transient が丸ごと delay に取り残されて波形 /
/// スペクトラム / ビート解析から欠ける (review-v2.3.0 hunt P2)。EOF 時に一度だけ
/// 呼び、出力が空になるまで flush を繰り返す。再生系 (video/decoder.rs) はストリーミング
/// 途中で flush できないので従来どおり (この関数は解析用デコーダ専用)。
fn flush_resampler(resampler: &mut ResampleContext, out: &mut Vec<f32>) -> Result<(), String> {
    loop {
        let delay_out = resampler
            .delay()
            .map(|d| d.output.max(0) as u64)
            .unwrap_or(0);
        if delay_out == 0 {
            return Ok(());
        }
        let out_cap = (delay_out + SWR_OUTPUT_SAFETY_SAMPLES) as usize;
        let mut resampled = AudioFrame::empty();
        unsafe {
            resampled.alloc(
                Sample::F32(SampleType::Packed),
                out_cap,
                ffmpeg::ChannelLayoutMask::STEREO,
            );
            resampled.set_rate(OUT_RATE);
        }
        if let Err(e) = resampler.flush(&mut resampled) {
            // flush 失敗は末尾数十サンプルの欠落に留まるので致命的でない。
            crate::logger::log(format!("[audio] swr flush: {e}"));
            return Ok(());
        }
        let nb = resampled.samples();
        if nb == 0 {
            return Ok(());
        }
        let count = nb * OUT_CHANNELS;
        if out.try_reserve(count).is_err() {
            return Err(format!(
                "音声デコードのメモリ確保に失敗しました (ファイルが長すぎる可能性: {} samples)",
                out.len() + count
            ));
        }
        unsafe {
            let ptr = (*resampled.as_ptr()).data[0] as *const f32;
            if ptr.is_null() {
                return Ok(());
            }
            out.extend_from_slice(std::slice::from_raw_parts(ptr, count));
        }
    }
}

/// 1 音声フレームを 48kHz stereo f32 packed に resample して `out` に追記する。
///
/// swresample の内部バッファリング (位相補間の遅延) は「入力フレームごとに 1 回 run」で
/// 進め、EOF 時に `flush_resampler` で吐き切る (review-v2.3.0 hunt P2)。
fn append_resampled(
    resampler: &mut ResampleContext,
    frame: &mut AudioFrame,
    out: &mut Vec<f32>,
    in_rate: u32,
) -> Result<(), String> {
    let in_samples = frame.samples();
    if in_samples == 0 {
        return Ok(());
    }
    // フレーム単位のレイアウト正規化 (Codex P2): resampler は正規化済みレイアウトで
    // 構築されているが、各フレームの ch_layout が UNSPEC のままだと swr_convert_frame が
    // 失敗する (古い WMA 等)。マスクを持たないフレームだけ差し替える
    // (video/decoder.rs:5432-5435 と同じ手順)。
    if frame.ch_layout().mask().is_none() {
        let normalized = normalize_layout(frame.ch_layout());
        frame.set_ch_layout(normalized);
    }
    // 出力バッファは標準 FFmpeg パターン ceil(in * out_rate / in_rate) + swr delay + safety
    // で確保する (Codex P2)。floor + 固定 64 だと downsample や delay > 64 のときに
    // サンプルが swr 内部 delay に取り残されたり run() error になる
    // (video/decoder.rs:5376 resample_output_buffer_samples と同式)。
    let delay_out = resampler
        .delay()
        .map(|d| d.output.max(0) as u64)
        .unwrap_or(0);
    let rate_converted = (in_samples as u64 * OUT_RATE as u64).div_ceil(in_rate.max(1) as u64);
    let out_cap = (rate_converted + delay_out + SWR_OUTPUT_SAFETY_SAMPLES) as usize;
    let mut resampled = AudioFrame::empty();
    unsafe {
        resampled.alloc(
            Sample::F32(SampleType::Packed),
            out_cap,
            ffmpeg::ChannelLayoutMask::STEREO,
        );
        resampled.set_rate(OUT_RATE);
    }
    if let Err(e) = resampler.run(frame, &mut resampled) {
        // 1 フレームの resample 失敗は致命的でない。
        crate::logger::log(format!("[audio] swr resample: {e}"));
        return Ok(());
    }
    let nb = resampled.samples();
    if nb == 0 {
        return Ok(());
    }
    debug_assert_eq!(resampled.format(), Sample::F32(SampleType::Packed));
    debug_assert!(resampled.is_packed());
    // linesize パディングを避けるため data[0] を直接 f32 スライス化する
    // (video/decoder.rs と同じ手順)。
    let count = nb * OUT_CHANNELS;
    // 長尺 / 巨大ファイルで Vec が GB 級に膨らんでも abort させず、確保失敗は Err で
    // 上位ワーカーへ返す (Codex P2: extend_from_slice の暗黙 abort 回避)。
    if out.try_reserve(count).is_err() {
        return Err(format!(
            "音声デコードのメモリ確保に失敗しました (ファイルが長すぎる可能性: {} samples)",
            out.len() + count
        ));
    }
    unsafe {
        let ptr = (*resampled.as_ptr()).data[0] as *const f32;
        if ptr.is_null() {
            return Ok(());
        }
        out.extend_from_slice(std::slice::from_raw_parts(ptr, count));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_threshold_is_geometric() {
        let initial = 96_000; // 2 秒 @ 48kHz
        // 初回 emit 前は initial。
        assert_eq!(next_partial_threshold_frames(0, initial), initial);
        // 以降は倍々。
        assert_eq!(next_partial_threshold_frames(initial, initial), initial * 2);
        assert_eq!(
            next_partial_threshold_frames(initial * 2, initial),
            initial * 4
        );
        assert_eq!(
            next_partial_threshold_frames(initial * 4, initial),
            initial * 8
        );
    }

    #[test]
    fn partial_threshold_initial_never_zero() {
        // initial=0 でも 1 を返し、以降 saturating_mul で無限ループにならない。
        assert_eq!(next_partial_threshold_frames(0, 0), 1);
        assert_eq!(next_partial_threshold_frames(1, 0), 2);
    }

    #[test]
    fn partial_threshold_total_work_bounded_by_2x() {
        // 幾何級数の閾値を全長の半分まで積み上げ、partial 解析総 frame 数が全長の約 2 倍以内で
        // 頭打ちになることを確認する（50% 抑制 + 倍々スケジュールの効果）。
        let initial = 96_000usize;
        let total_frames = 48_000 * 60 * 60; // 1 時間
        let half = total_frames / 2;
        let mut last = 0usize;
        let mut sum_prefix = 0usize;
        loop {
            let threshold = next_partial_threshold_frames(last, initial);
            if threshold >= half {
                break;
            }
            // partial は threshold 到達時点のプレフィックス（≈ threshold frame）を解析する。
            sum_prefix += threshold;
            last = threshold;
        }
        // partial 総解析量 < total（50% 抑制で最後の partial < half、倍々なので総和 < 2*half = total）。
        assert!(
            sum_prefix < total_frames,
            "partial 総解析 {sum_prefix} が全長 {total_frames} を超えている"
        );
    }
}
