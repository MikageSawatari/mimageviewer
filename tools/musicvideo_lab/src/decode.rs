use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use music_core::resample_linear_stereo;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

pub const OUTPUT_SAMPLE_RATE: u32 = 48_000;

#[derive(Debug)]
pub struct AudioData {
    pub stereo_pcm: Vec<f32>,
    pub sample_rate: u32,
    pub duration_secs: f64,
}

pub fn decode_audio(path: &Path, cancel: &AtomicBool) -> Result<AudioData, String> {
    let file = std::fs::File::open(path)
        .map_err(|error| format!("音声ファイルを開けません ({}): {error}", path.display()))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        hint.with_extension(extension);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|error| format!("音声形式を判定できません: {error}"))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|track| {
            track.codec_params.codec != CODEC_TYPE_NULL
                && track.codec_params.sample_rate.is_some()
                && track.codec_params.channels.is_some()
        })
        .or_else(|| {
            format
                .tracks()
                .iter()
                .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        })
        .ok_or_else(|| "デコード可能な音声トラックがありません".to_owned())?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|error| format!("音声デコーダーを作成できません: {error}"))?;

    let mut source_rate = 0;
    let mut stereo = Vec::new();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("音声の読み込みをキャンセルしました".to_owned());
        }
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(_)) => break,
            Err(SymphoniaError::ResetRequired) => {
                return Err("音声デコーダーの再初期化が必要です".to_owned());
            }
            Err(error) => return Err(format!("音声パケットを読めません: {error}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(error) => return Err(format!("音声をデコードできません: {error}")),
        };
        let spec = *decoded.spec();
        if source_rate != 0 && source_rate != spec.rate {
            return Err("途中でサンプルレートが変わる音声には未対応です".to_owned());
        }
        source_rate = spec.rate;
        let channels = spec.channels.count().max(1);
        let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        samples.copy_interleaved_ref(decoded);
        for frame in samples.samples().chunks(channels) {
            let clean = |value: f32| {
                if value.is_finite() {
                    value.clamp(-1.0, 1.0)
                } else {
                    0.0
                }
            };
            let left = clean(frame.first().copied().unwrap_or(0.0));
            let right = clean(frame.get(1).copied().unwrap_or(left));
            stereo.extend_from_slice(&[left, right]);
        }
    }
    if source_rate == 0 || stereo.is_empty() {
        return Err("音声サンプルをデコードできませんでした".to_owned());
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("音声の読み込みをキャンセルしました".to_owned());
    }
    let stereo_pcm = if source_rate == OUTPUT_SAMPLE_RATE {
        stereo
    } else {
        resample_linear_stereo(&stereo, source_rate, OUTPUT_SAMPLE_RATE)
    };
    let duration_secs = stereo_pcm.len() as f64 / 2.0 / OUTPUT_SAMPLE_RATE as f64;
    Ok(AudioData {
        stereo_pcm,
        sample_rate: OUTPUT_SAMPLE_RATE,
        duration_secs,
    })
}
